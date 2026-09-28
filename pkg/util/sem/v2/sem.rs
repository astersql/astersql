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

// SEM（Security Enhanced Mode）v2 运行时核心。
//
// 维护进程级 `globalSem` 指针，提供 Enable/Disable 与 schema/表/系统变量/状态变量/
// 权限/SQL 可见性与限制查询。配置经 `buildSEMFromConfig` 编译为 `SemImpl` 后生效。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

use crate::{
    Config, SQLRestriction, SQLRule, parseSEMConfigFromFile, semCommand, sqlRuleNameMap,
    validateSEMConfig,
};

/// 受限 SQL 判定闭包：对 AST 节点返回 true 表示该语句受限。
type SEMSQLValidateFn = Arc<dyn Fn(&dyn ast::Node) -> bool + Send + Sync>;

/// 进程级 SEM 实例指针的原子式封装（内部用 RwLock 保护 Option）。
pub(crate) struct AtomicSemPointer {
    inner: RwLock<Option<Arc<SemImpl>>>,
}

impl AtomicSemPointer {
    fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    #[allow(non_snake_case)]
    /// 读取当前已启用的 SEM 实例；未启用时返回 None。
    pub(crate) fn Load(&self) -> Option<Arc<SemImpl>> {
        self.inner
            .read()
            .expect("SEM pointer lock poisoned")
            .clone()
    }

    #[allow(non_snake_case)]
    /// 写入或清空全局 SEM 实例。
    fn Store(&self, sem: Option<Arc<SemImpl>>) {
        *self.inner.write().expect("SEM pointer lock poisoned") = sem;
    }
}

#[allow(non_upper_case_globals)]
/// 全局 SEM 单例指针；Enable 时 Store，Disable 时清空。
pub(crate) static globalSem: LazyLock<AtomicSemPointer> = LazyLock::new(AtomicSemPointer::new);

#[allow(non_snake_case)]
/// 库名是否在受限 schema 列表中（大小写不敏感比较前转小写）。
pub fn IsInvisibleSchema(dbName: &str) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isInvisibleSchema(dbName))
}

#[allow(non_snake_case)]
/// 表是否不可见：所属 schema 受限，或该表被配置为 hidden。
pub fn IsInvisibleTable(dbLowerName: &str, tblLowerName: &str) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isInvisibleTable(dbLowerName, tblLowerName))
}

#[allow(non_snake_case)]
/// 权限名是否受限（调用方须已转大写）；含 RESTRICTED_ 前缀或配置列表命中。
pub fn IsRestrictedPrivilege(privilege: &str) -> bool {
    intest::Assert(
        privilege.to_uppercase() == privilege,
        &["privilege name must be uppercase".into()],
    );
    globalSem
        .Load()
        .is_some_and(|sem| sem.isRestrictedPrivilege(privilege))
}

#[allow(non_snake_case)]
/// 系统变量是否对普通用户隐藏。
pub fn IsInvisibleSysVar(varName: &str) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isInvisibleSysVar(varName))
}

#[allow(non_snake_case)]
/// 系统变量是否被 SEM 标记为只读。
pub fn IsReadOnlyVariable(varName: &str) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isReadOnlyVariable(varName))
}

#[allow(non_snake_case)]
/// 状态变量（SHOW STATUS）是否不可见。
pub fn IsInvisibleStatusVar(varName: &str) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isInvisibleStatusVar(varName))
}

#[allow(non_snake_case)]
/// 语句是否命中受限 SQL 命令名或命名规则。
pub fn IsRestrictedSQL(stmt: &dyn ast::Node) -> bool {
    globalSem
        .Load()
        .is_some_and(|sem| sem.isRestrictedSQL(stmt))
}

#[allow(non_snake_case)]
/// 从配置文件路径解析并启用 SEM（要求当前未启用）。
pub fn Enable(configPath: &str) -> Result<(), String> {
    intest::Assert(
        globalSem.Load().is_none(),
        &["SEM is already enabled".into()],
    );
    let config = parseSEMConfigFromFile(configPath)?;
    EnableBy(&config)
}

#[allow(non_snake_case)]
/// 用已解析的 Config 校验、构建并启用 SEM，同时置位增强安全开关。
pub fn EnableBy(semConfig: &Config) -> Result<(), String> {
    intest::Assert(
        globalSem.Load().is_none(),
        &["SEM is already enabled".into()],
    );
    validateSEMConfig(semConfig)?;

    // 先按配置覆盖系统变量，再挂到全局指针，最后标记增强安全已启用。
    let sem = buildSEMFromConfig(semConfig);
    sem.overrideRestrictedVariable();
    globalSem.Store(Some(sem));

    let _ = variable::SetSysVar(vardef::TiDBEnableEnhancedSecurity, "CONFIG");
    logutil::log::BgLogger()
        .info("tidb-server is operating with security enhanced mode (SEM) v2 enabled");
    Ok(())
}

#[allow(non_snake_case)]
/// SEM 是否已启用（globalSem 非空）。
pub fn IsEnabled() -> bool {
    globalSem.Load().is_some()
}

#[allow(non_snake_case)]
/// 关闭 SEM 并将增强安全开关设为 Off。
pub fn Disable() {
    globalSem.Store(None);
    let _ = variable::SetSysVar(vardef::TiDBEnableEnhancedSecurity, vardef::Off);
}

/// SEM 运行时实现：持有各类受限集合与可选 SQL 校验闭包。
pub(crate) struct SemImpl {
    restrictedDatabases: HashSet<String>,
    restrictedTables: HashMap<String, HashMap<String, RestrictedTableAttr>>,
    restrictedVariables: HashMap<String, RestrictedVariableAttr>,
    pub(crate) restrictedPrivileges: RwLock<HashSet<String>>,
    restrictedStatusVariables: HashSet<String>,
    restrictedSQL: Option<SEMSQLValidateFn>,
    pub(crate) restrictedHints: HashSet<String>,
}

/// 受限系统变量属性：隐藏、只读、强制覆盖值。
struct RestrictedVariableAttr {
    hidden: bool,
    readonly: bool,
    value: String,
}

/// 受限表属性：当前仅记录是否 hidden。
struct RestrictedTableAttr {
    hidden: bool,
}

impl SemImpl {
    #[allow(non_snake_case)]
    /// schema 名转小写后是否在 restrictedDatabases 中。
    pub(crate) fn isInvisibleSchema(&self, dbName: &str) -> bool {
        self.restrictedDatabases.contains(&dbName.to_lowercase())
    }

    #[allow(non_snake_case)]
    /// schema 受限则其下所有表不可见；否则查表级 hidden。
    pub(crate) fn isInvisibleTable(&self, dbLowerName: &str, tblLowerName: &str) -> bool {
        if self.isInvisibleSchema(dbLowerName) {
            return true;
        }
        self.restrictedTables
            .get(dbLowerName)
            .and_then(|tables| tables.get(tblLowerName))
            .is_some_and(|table| table.hidden)
    }

    #[allow(non_snake_case)]
    /// RESTRICTED_ 前缀权限始终受限，其余查配置集合。
    pub(crate) fn isRestrictedPrivilege(&self, privilege: &str) -> bool {
        privilege.starts_with("RESTRICTED_")
            || self
                .restrictedPrivileges
                .read()
                .expect("SEM privilege lock poisoned")
                .contains(privilege)
    }

    #[allow(non_snake_case)]
    /// 变量属性中 hidden=true 则不可见。
    pub(crate) fn isInvisibleSysVar(&self, varName: &str) -> bool {
        self.restrictedVariables
            .get(varName)
            .is_some_and(|attr| attr.hidden)
    }

    #[allow(non_snake_case)]
    /// 状态变量名是否在受限集合中。
    fn isInvisibleStatusVar(&self, varName: &str) -> bool {
        self.restrictedStatusVariables.contains(varName)
    }

    #[allow(non_snake_case)]
    /// 变量属性中 readonly=true 则只读。
    pub(crate) fn isReadOnlyVariable(&self, varName: &str) -> bool {
        self.restrictedVariables
            .get(varName)
            .is_some_and(|attr| attr.readonly)
    }

    #[allow(non_snake_case)]
    /// 若存在 SQL 校验闭包则调用之。
    fn isRestrictedSQL(&self, stmt: &dyn ast::Node) -> bool {
        self.restrictedSQL
            .as_ref()
            .is_some_and(|restricted| restricted(stmt))
    }

    #[allow(non_snake_case)]
    /// 将配置中非空 Value 写入系统变量注册表（强制覆盖）。
    pub(crate) fn overrideRestrictedVariable(&self) {
        for (name, attr) in &self.restrictedVariables {
            if !attr.value.is_empty() {
                let _ = variable::SetSysVar(name, &attr.value);
            }
        }
    }
}

#[allow(non_snake_case)]
/// 根据 SQLRestriction 编译“命令名集合 ∪ 命名规则”判定闭包；无配置则返回 None。
fn buildSEMSqlValidateFunction(
    sqlRestriction: Option<&SQLRestriction>,
) -> Option<SEMSQLValidateFn> {
    let sqlRestriction = sqlRestriction?;
    // 规则名必须能在 sqlRuleNameMap 中解析；未知规则打日志并 Assert。
    let mut sqlRules: HashMap<String, SQLRule> = HashMap::with_capacity(sqlRestriction.Rule.len());
    for ruleName in &sqlRestriction.Rule {
        if let Some(rule) = sqlRuleNameMap.get(ruleName.as_str()).copied() {
            sqlRules.insert(ruleName.clone(), rule);
        } else {
            logutil::log::BgLogger().warn(format!("unknown SQL rule: {ruleName}"));
            intest::Assert(
                false,
                &["unknown SQL rule: %s".into(), ruleName.clone().into()],
            );
        }
    }

    // 命令名统一 trim+大写；空串过滤掉。
    let sqlCommands: HashSet<String> = sqlRestriction
        .SQL
        .iter()
        .map(|sql| sql.trim().to_uppercase())
        .filter(|sql| !sql.is_empty())
        .collect();

    Some(Arc::new(move |stmt: &dyn ast::Node| {
        sqlCommands.contains(&semCommand(stmt)) || sqlRules.values().any(|rule| rule(stmt))
    }))
}

#[allow(non_snake_case)]
/// 将 Config 编译为运行时 SemImpl（库/表/变量/权限/hint/SQL 规则）。
pub(crate) fn buildSEMFromConfig(cfg: &Config) -> Arc<SemImpl> {
    // schema -> (table -> attr)；同 schema 多表合并进内层 map。
    let mut restrictedTables: HashMap<String, HashMap<String, RestrictedTableAttr>> =
        HashMap::new();
    for table in &cfg.RestrictedTables {
        restrictedTables
            .entry(table.Schema.clone())
            .or_default()
            .insert(
                table.Name.clone(),
                RestrictedTableAttr {
                    hidden: table.Hidden,
                },
            );
    }

    let restrictedVariables = cfg
        .RestrictedVariables
        .iter()
        .map(|variable| {
            (
                variable.Name.clone(),
                RestrictedVariableAttr {
                    hidden: variable.Hidden,
                    readonly: variable.Readonly,
                    value: variable.Value.clone(),
                },
            )
        })
        .collect();

    Arc::new(SemImpl {
        restrictedDatabases: cfg.RestrictedDatabases.iter().cloned().collect(),
        restrictedTables,
        restrictedVariables,
        restrictedStatusVariables: cfg.RestrictedStatusVar.iter().cloned().collect(),
        restrictedPrivileges: RwLock::new(
            cfg.RestrictedPrivileges
                .iter()
                .map(|privilege| privilege.to_uppercase())
                .collect(),
        ),
        restrictedHints: cfg
            .RestrictedHints
            .iter()
            .map(|hint| hint.to_lowercase())
            .collect(),
        restrictedSQL: buildSEMSqlValidateFunction(Some(&cfg.RestrictedSQL)),
    })
}
