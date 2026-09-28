// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Rust equivalents of the self-contained AST behavior in `misc.go`.

// 杂项语句 AST 与还原/脱敏逻辑（对照 `misc.go`）。
//
// 涵盖事务、准备语句、用户/权限、PLAN REPLAYER、BRIE、流量回放、
// QUERY WATCH、ADMIN 等非核心 DML/DDL 语句，以及 SensitiveStatement 契约。

use std::collections::BTreeMap;

/// 事务隔离级别 READ-COMMITTED。
pub const READ_COMMITTED: &str = "READ-COMMITTED";
/// 事务隔离级别 READ-UNCOMMITTED。
pub const READ_UNCOMMITTED: &str = "READ-UNCOMMITTED";
/// 事务隔离级别 SERIALIZABLE。
pub const SERIALIZABLE: &str = "SERIALIZABLE";
/// 事务隔离级别 REPEATABLE-READ。
pub const REPEATABLE_READ: &str = "REPEATABLE-READ";
/// 乐观事务模式标识。
pub const OPTIMISTIC: &str = "OPTIMISTIC";
/// 悲观事务模式标识。
pub const PESSIMISTIC: &str = "PESSIMISTIC";

/// 数值类型 UNSIGNED/ZEROFILL 选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeOpt {
    pub is_unsigned: bool,
    pub is_zerofill: bool,
}

/// 浮点类型长度与小数位选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FloatOpt {
    pub flen: i32,
    pub decimal: i32,
}

/// IDENTIFIED BY/AS/WITH 认证选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthOption {
    pub by_auth_string: bool,
    pub auth_string: String,
    pub by_hash_string: bool,
    pub hash_string: String,
    pub auth_plugin: String,
}

impl AuthOption {
    pub fn restore(&self) -> String {
        let mut sql = String::from("IDENTIFIED");
        if !self.auth_plugin.is_empty() {
            sql.push_str(" WITH ");
            sql.push_str(&quote_string(&self.auth_plugin));
        }
        if self.by_auth_string {
            sql.push_str(" BY ");
            sql.push_str(&quote_string(&self.auth_string));
        } else if self.by_hash_string {
            sql.push_str(" AS ");
            sql.push_str(&quote_string(&self.hash_string));
        }
        sql
    }
}

/// PLAN REPLAYER 语句：转储/加载/捕获执行计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanReplayerStmt {
    pub statement: Option<String>,
    pub analyze: bool,
    pub load: bool,
    pub capture: bool,
    pub remove: bool,
    pub sql_digest: String,
    pub plan_digest: String,
    pub file: String,
    pub statements: Vec<String>,
    pub historical_stats: Option<String>,
    pub where_clause: Option<String>,
    pub order_by: Option<String>,
    pub limit: Option<String>,
}

impl PlanReplayerStmt {
    pub fn load(file: impl Into<String>) -> Self {
        Self {
            load: true,
            file: file.into(),
            ..Self::default()
        }
    }

    pub fn capture(sql_digest: impl Into<String>, plan_digest: impl Into<String>) -> Self {
        Self {
            capture: true,
            sql_digest: sql_digest.into(),
            plan_digest: plan_digest.into(),
            ..Self::default()
        }
    }

    pub fn remove(sql_digest: impl Into<String>, plan_digest: impl Into<String>) -> Self {
        Self {
            remove: true,
            sql_digest: sql_digest.into(),
            plan_digest: plan_digest.into(),
            ..Self::default()
        }
    }

    pub fn dump_statements<I, S>(analyze: bool, statements: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            analyze,
            statements: statements.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn dump_slow_query() -> Self {
        Self::default()
    }

    pub fn restore(&self) -> String {
        if self.load {
            return format!("PLAN REPLAYER LOAD {}", quote_string(&self.file));
        }
        if self.capture {
            return format!(
                "PLAN REPLAYER CAPTURE {} {}",
                quote_string(&self.sql_digest),
                quote_string(&self.plan_digest)
            );
        }
        if self.remove {
            return format!(
                "PLAN REPLAYER CAPTURE REMOVE {} {}",
                quote_string(&self.sql_digest),
                quote_string(&self.plan_digest)
            );
        }

        let mut sql = String::from("PLAN REPLAYER DUMP ");
        if let Some(stats) = &self.historical_stats {
            sql.push_str("WITH STATS ");
            sql.push_str(stats);
            sql.push(' ');
        }
        sql.push_str(if self.analyze {
            "EXPLAIN ANALYZE "
        } else {
            "EXPLAIN "
        });
        if let Some(statement) = &self.statement {
            sql.push_str(statement);
        } else if self.statements.is_empty() {
            if self.file.is_empty() {
                sql.push_str("SLOW QUERY");
                if let Some(value) = &self.where_clause {
                    sql.push_str(" WHERE ");
                    sql.push_str(value);
                }
                if let Some(value) = &self.order_by {
                    sql.push(' ');
                    sql.push_str(value);
                }
                if let Some(value) = &self.limit {
                    sql.push(' ');
                    sql.push_str(value);
                }
            } else {
                sql.push_str(&quote_string(&self.file));
            }
        } else {
            sql.push('(');
            sql.push_str(
                &self
                    .statements
                    .iter()
                    .map(|statement| quote_string(statement))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            sql.push(')');
        }
        sql
    }
}

/// 提示中的表引用（库名、表名、查询块、分区）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintTable {
    pub db_name: String,
    pub table_name: String,
    pub qb_name: String,
    pub partitions: Vec<String>,
}

impl HintTable {
    pub fn new(db_name: impl Into<String>, table_name: impl Into<String>) -> Self {
        Self {
            db_name: db_name.into(),
            table_name: table_name.into(),
            ..Self::default()
        }
    }

    pub fn bare(table_name: impl Into<String>) -> Self {
        Self::new("", table_name)
    }

    fn restore(&self) -> String {
        let mut sql = if self.db_name.is_empty() {
            quote_name(&self.table_name)
        } else {
            format!(
                "{}.{}",
                quote_name(&self.db_name),
                quote_name(&self.table_name)
            )
        };
        if !self.qb_name.is_empty() {
            sql.push('@');
            sql.push_str(&quote_name(&self.qb_name));
        }
        if !self.partitions.is_empty() {
            sql.push_str(" PARTITION(");
            sql.push_str(
                &self
                    .partitions
                    .iter()
                    .map(|name| quote_name(name))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            sql.push(')');
        }
        sql
    }
}

/// LEADING 列表元素：单表或嵌套子列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeadingItem {
    Table(HintTable),
    List(LeadingList),
}

impl LeadingItem {
    pub fn table(table: HintTable) -> Self {
        Self::Table(table)
    }
    pub fn list(list: LeadingList) -> Self {
        Self::List(list)
    }
}

/// LEADING 提示中的表顺序列表，可嵌套。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LeadingList {
    pub items: Vec<LeadingItem>,
}

impl LeadingList {
    pub fn new(items: Vec<LeadingItem>) -> Self {
        Self { items }
    }

    pub fn flatten(&self) -> Vec<HintTable> {
        let mut tables = Vec::new();
        self.flatten_into(&mut tables);
        tables
    }

    fn flatten_into(&self, tables: &mut Vec<HintTable>) {
        for item in &self.items {
            match item {
                LeadingItem::Table(table) => tables.push(table.clone()),
                LeadingItem::List(list) => list.flatten_into(tables),
            }
        }
    }

    pub fn restore_with_qb(&self, qb_name: Option<&str>, qb_on_table: bool) -> String {
        self.restore_inner(qb_name.unwrap_or_default(), false, qb_on_table)
    }

    fn restore_inner(&self, qb_name: &str, need_parens: bool, qb_on_table: bool) -> String {
        let mut current_qb = qb_name;
        let mut parts = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            match item {
                LeadingItem::Table(table) => {
                    let mut restored = String::new();
                    if index == 0 && !current_qb.is_empty() && !qb_on_table {
                        restored.push('@');
                        restored.push_str(&quote_name(current_qb));
                        restored.push(' ');
                        current_qb = "";
                    }
                    restored.push_str(&table.restore());
                    parts.push(restored);
                }
                LeadingItem::List(list) => {
                    parts.push(list.restore_inner(current_qb, true, qb_on_table));
                    current_qb = "";
                }
            }
        }
        let restored = parts.join(", ");
        if need_parens && !restored.is_empty() {
            format!("({restored})")
        } else {
            restored
        }
    }
}

/// 查询监视选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryWatchOptionType {
    ResourceGroup,
    Action,
    Watch,
}

/// 查询监视选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryWatchOption {
    pub option_type: QueryWatchOptionType,
    pub value: String,
}

impl QueryWatchOption {
    pub fn resource_group(name: impl Into<String>) -> Self {
        Self {
            option_type: QueryWatchOptionType::ResourceGroup,
            value: name.into(),
        }
    }
    pub fn kill() -> Self {
        Self {
            option_type: QueryWatchOptionType::Action,
            value: "KILL".into(),
        }
    }
    pub fn cooldown() -> Self {
        Self {
            option_type: QueryWatchOptionType::Action,
            value: "COOLDOWN".into(),
        }
    }
    pub fn sql_text_exact(text: impl Into<String>) -> Self {
        Self {
            option_type: QueryWatchOptionType::Watch,
            value: format!("SQL TEXT EXACT TO {}", quote_string(&text.into())),
        }
    }

    pub fn restore(&self) -> String {
        match self.option_type {
            QueryWatchOptionType::ResourceGroup => {
                format!("RESOURCE GROUP {}", quote_name(&self.value))
            }
            QueryWatchOptionType::Action => format!("ACTION = {}", self.value),
            QueryWatchOptionType::Watch => self.value.clone(),
        }
    }
}

/// 检查 QUERY WATCH 选项追加是否与已有选项冲突。
pub fn check_query_watch_append(
    options: &[QueryWatchOption],
    new_option: &QueryWatchOption,
) -> bool {
    options
        .iter()
        .all(|option| option.option_type != new_option.option_type)
}

/// SET PASSWORD 语句 AST。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetPwdStmt {
    pub user: Option<(String, String)>,
    pub password: String,
    pub retain_current_password: bool,
}

impl SetPwdStmt {
    pub fn current_user(password: impl Into<String>, retain_current_password: bool) -> Self {
        Self {
            user: None,
            password: password.into(),
            retain_current_password,
        }
    }

    pub fn named(
        username: impl Into<String>,
        hostname: impl Into<String>,
        password: impl Into<String>,
        retain_current_password: bool,
    ) -> Self {
        Self {
            user: Some((username.into(), hostname.into())),
            password: password.into(),
            retain_current_password,
        }
    }

    pub fn secure_text(&self) -> String {
        let mut text = String::from("set password");
        if let Some((username, hostname)) = &self.user {
            text.push_str(" for user ");
            text.push_str(username);
            text.push('@');
            text.push_str(hostname);
        }
        if self.retain_current_password {
            text.push_str(" RETAIN CURRENT PASSWORD");
        }
        text
    }
}

/// 按方案与敏感查询参数规则对 URL 做脱敏（打码密钥）。
// 仅对特定对象存储 scheme 的敏感查询参数打码，其它 URL 原样返回。
pub fn redact_url(input: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(input) else {
        return input.to_owned();
    };
    let sensitive_keys: &[&str] = match parsed.scheme().to_ascii_lowercase().as_str() {
        "s3" | "ks3" | "oss" => &["access-key", "secret-access-key", "session-token"],
        "azure" | "azblob" => &["account-key", "encryption-key", "sas-token"],
        _ => return input.to_owned(),
    };

    // Go's url.Values.Encode sorts keys and preserves all values for a key.
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in parsed.query_pairs() {
        values
            .entry(key.into_owned())
            .or_default()
            .push(value.into_owned());
    }
    for (key, entries) in &mut values {
        let normalized = key.to_ascii_lowercase().replace('_', "-");
        if sensitive_keys.contains(&normalized.as_str()) {
            entries.clear();
            entries.push("xxxxxx".to_owned());
        }
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, entries) in values {
        for value in entries {
            serializer.append_pair(&key, &value);
        }
    }
    let query = serializer.finish();
    parsed.set_query((!query.is_empty()).then_some(query.as_str()));
    parsed.to_string()
}

/// 按 MySQL 标识符规则用反引号引用并转义。
fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// 按 SQL 字符串字面量规则加单引号并转义。
fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// PREPARE 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrepareStmt {
    pub name: String,
    pub sql_text: String,
    pub sql_var: Option<String>,
}

impl PrepareStmt {
    pub fn restore(&self) -> Result<String, String> {
        let source = if !self.sql_text.is_empty() {
            quote_string(&self.sql_text)
        } else if let Some(variable) = &self.sql_var {
            variable.clone()
        } else {
            return Err("An error occurred while restore PrepareStmt".into());
        };
        Ok(format!("PREPARE {} FROM {source}", quote_name(&self.name)))
    }
}

/// DEALLOCATE语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeallocateStmt {
    pub name: String,
}
impl DeallocateStmt {
    pub fn restore(&self) -> String {
        format!("DEALLOCATE PREPARE {}", quote_name(&self.name))
    }
}

/// 已准备结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Prepared {
    pub stmt: String,
    pub stmt_type: String,
}

/// EXECUTE 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecuteStmt {
    pub name: String,
    pub using_vars: Vec<String>,
    pub binary_args: Vec<u8>,
    pub prep_stmt_id: u32,
    pub idx_in_multi: i32,
    pub from_general_stmt: bool,
}
impl ExecuteStmt {
    pub fn restore(&self) -> String {
        let mut sql = format!("EXECUTE {}", quote_name(&self.name));
        if !self.using_vars.is_empty() {
            sql.push_str(" USING ");
            sql.push_str(&self.using_vars.join(","));
        }
        sql
    }
}

/// BEGIN/START TRANSACTION 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BeginStmt {
    pub mode: String,
    pub causal_consistency_only: bool,
    pub read_only: bool,
    pub as_of: Option<String>,
}
impl BeginStmt {
    pub fn restore(&self) -> String {
        if !self.mode.is_empty() {
            return format!("BEGIN {}", self.mode.to_uppercase());
        }
        if self.read_only {
            let mut sql = "START TRANSACTION READ ONLY".to_owned();
            if let Some(as_of) = &self.as_of {
                sql.push(' ');
                sql.push_str(as_of);
            }
            return sql;
        }
        if self.causal_consistency_only {
            "START TRANSACTION WITH CAUSAL CONSISTENCY ONLY".into()
        } else {
            "START TRANSACTION".into()
        }
    }
}

/// BINLOG语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinlogStmt {
    pub value: String,
}
impl BinlogStmt {
    pub fn restore(&self) -> String {
        format!("BINLOG {}", quote_string(&self.value))
    }
}

/// 事务完成类型（DEFAULT/CHAIN/RELEASE）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompletionType {
    #[default]
    Default,
    Chain,
    Release,
}
impl CompletionType {
    pub fn restore(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Chain => " AND CHAIN",
            Self::Release => " RELEASE",
        }
    }
}

/// COMMIT 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommitStmt {
    pub completion_type: CompletionType,
}
impl CommitStmt {
    pub fn restore(&self) -> String {
        format!("COMMIT{}", self.completion_type.restore())
    }
}

/// ROLLBACK 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RollbackStmt {
    pub completion_type: CompletionType,
    pub savepoint_name: String,
}
impl RollbackStmt {
    pub fn restore(&self) -> String {
        let savepoint = if self.savepoint_name.is_empty() {
            String::new()
        } else {
            format!(" TO {}", self.savepoint_name)
        };
        format!("ROLLBACK{savepoint}{}", self.completion_type.restore())
    }
}

/// USE语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UseStmt {
    pub db_name: String,
}
impl UseStmt {
    pub fn restore(&self) -> String {
        format!("USE {}", quote_name(&self.db_name))
    }
}

/// NAMES常量。
pub const SET_NAMES: &str = "SetNAMES";
/// CHARSET常量。
pub const SET_CHARSET: &str = "SetCharset";
/// URI常量。
pub const TIDB_CLOUD_STORAGE_URI: &str = "tidb_cloud_storage_uri";
/// URI常量。
pub const CLOUD_STORAGE_URI: &str = "cloud_storage_uri";

/// 变量赋值结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariableAssignment {
    pub name: String,
    pub value: String,
    pub is_instance: bool,
    pub is_global: bool,
    pub is_system: bool,
    pub extend_value: Option<String>,
}
impl VariableAssignment {
    pub fn restore(&self) -> String {
        let mut sql = String::new();
        if self.is_system {
            sql.push_str("@@");
            sql.push_str(if self.is_global {
                "GLOBAL"
            } else if self.is_instance {
                "INSTANCE"
            } else {
                "SESSION"
            });
            sql.push('.');
        } else if self.name != SET_NAMES && self.name != SET_CHARSET {
            sql.push('@');
        }
        match self.name.as_str() {
            SET_NAMES => sql.push_str("NAMES "),
            SET_CHARSET => sql.push_str("CHARSET "),
            _ => {
                sql.push_str(&quote_name(&self.name));
                sql.push('=');
            }
        }
        if self.name == TIDB_CLOUD_STORAGE_URI {
            sql.push_str(&redact_url(&self.value));
        } else {
            sql.push_str(&self.value);
        }
        if let Some(collation) = &self.extend_value {
            sql.push_str(" COLLATE ");
            sql.push_str(collation);
        }
        sql
    }
}

/// SET 变量赋值语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetStmt {
    pub variables: Vec<VariableAssignment>,
}
impl SetStmt {
    pub fn restore(&self) -> String {
        format!(
            "SET {}",
            self.variables
                .iter()
                .map(VariableAssignment::restore)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
    pub fn secure_text(&self) -> String {
        self.restore()
    }
}

/// SET配置语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetConfigStmt {
    pub target_type: String,
    pub instance: String,
    pub name: String,
    pub value: String,
}
impl SetConfigStmt {
    pub fn restore(&self) -> String {
        let target = if self.target_type.is_empty() {
            quote_string(&self.instance)
        } else {
            self.target_type.to_uppercase()
        };
        format!(
            "SET CONFIG {target} {} = {}",
            self.name.to_uppercase(),
            self.value
        )
    }
}

/// SET会话状态语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetSessionStatesStmt {
    pub session_states: String,
}
impl SetSessionStatesStmt {
    pub fn restore(&self) -> String {
        format!("SET SESSION_STATES {}", quote_string(&self.session_states))
    }
}

/// SAVEPOINT语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SavepointStmt {
    pub name: String,
}
impl SavepointStmt {
    pub fn restore(&self) -> String {
        format!("SAVEPOINT {}", self.name)
    }
}

/// 释放SAVEPOINT语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReleaseSavepointStmt {
    pub name: String,
}
impl ReleaseSavepointStmt {
    pub fn restore(&self) -> String {
        format!("RELEASE SAVEPOINT {}", self.name)
    }
}

/// 用户/角色身份：用户名@主机。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identity {
    pub username: String,
    pub hostname: String,
}
impl Identity {
    pub fn new(username: impl Into<String>, hostname: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            hostname: hostname.into(),
        }
    }
    pub fn restore(&self) -> String {
        format!(
            "{}@{}",
            quote_string(&self.username),
            quote_string(&self.hostname)
        )
    }
}

/// SET角色语句类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SetRoleStmtType {
    #[default]
    Default,
    None,
    All,
    AllExcept,
    Regular,
}

/// SET角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetRoleStmt {
    pub option: SetRoleStmtType,
    pub roles: Vec<Identity>,
}
impl SetRoleStmt {
    pub fn restore(&self) -> String {
        let mut sql = String::from("SET ROLE");
        sql.push_str(match self.option {
            SetRoleStmtType::Default => " DEFAULT",
            SetRoleStmtType::None => " NONE",
            SetRoleStmtType::All => " ALL",
            SetRoleStmtType::AllExcept => " ALL EXCEPT",
            SetRoleStmtType::Regular => "",
        });
        if !self.roles.is_empty() {
            sql.push(' ');
            sql.push_str(
                &self
                    .roles
                    .iter()
                    .map(Identity::restore)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        sql
    }
}

/// SET默认角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetDefaultRoleStmt {
    pub option: SetRoleStmtType,
    pub roles: Vec<Identity>,
    pub users: Vec<Identity>,
}
impl SetDefaultRoleStmt {
    pub fn restore(&self) -> String {
        let mut sql = String::from("SET DEFAULT ROLE");
        sql.push_str(match self.option {
            SetRoleStmtType::None => " NONE",
            SetRoleStmtType::All => " ALL",
            _ => "",
        });
        if !self.roles.is_empty() {
            sql.push(' ');
            sql.push_str(
                &self
                    .roles
                    .iter()
                    .map(Identity::restore)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        sql.push_str(" TO");
        if !self.users.is_empty() {
            sql.push(' ');
            sql.push_str(
                &self
                    .users
                    .iter()
                    .map(Identity::restore)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        sql
    }
}

/// 双密码选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DualPasswordOptionType {
    #[default]
    None,
    RetainCurrent,
    DiscardOld,
}
impl DualPasswordOptionType {
    pub fn restore(self) -> Result<&'static str, String> {
        match self {
            Self::RetainCurrent => Ok("RETAIN CURRENT PASSWORD"),
            Self::DiscardOld => Ok("DISCARD OLD PASSWORD"),
            Self::None => Err("Unsupported DualPasswordOptionType 0".into()),
        }
    }
}

/// CREATE/ALTER USER 中的单个用户规格。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserSpec {
    pub user: Identity,
    pub auth: Option<AuthOption>,
    pub dual_password: DualPasswordOptionType,
    pub is_role: bool,
}
impl UserSpec {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = self.user.restore();
        if let Some(auth) = &self.auth {
            sql.push(' ');
            sql.push_str(&auth.restore());
        }
        if self.dual_password != DualPasswordOptionType::None {
            sql.push(' ');
            sql.push_str(self.dual_password.restore()?);
        }
        Ok(sql)
    }
    pub fn security_string(&self) -> String {
        let dual = match self.dual_password {
            DualPasswordOptionType::RetainCurrent => " RETAIN CURRENT PASSWORD",
            DualPasswordOptionType::DiscardOld => " DISCARD OLD PASSWORD",
            DualPasswordOptionType::None => "",
        };
        let has_password = self
            .auth
            .as_ref()
            .is_some_and(|a| !a.auth_string.is_empty() || !a.hash_string.is_empty());
        if has_password {
            format!("{{{} password = ***{dual}}}", self.user.restore())
        } else if !dual.is_empty() {
            format!("{{{}{dual}}}", self.user.restore())
        } else {
            self.user.restore()
        }
    }
}

/// 认证令牌或TLS选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AuthTokenOrTlsOptionType {
    #[default]
    None,
    Ssl,
    X509,
    Cipher,
    Issuer,
    Subject,
    San,
    TokenIssuer,
}
impl AuthTokenOrTlsOptionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Ssl => "SSL",
            Self::X509 => "X509",
            Self::Cipher => "CIPHER",
            Self::Issuer => "ISSUER",
            Self::Subject => "SUBJECT",
            Self::San => "SAN",
            Self::TokenIssuer => "TOKEN_ISSUER",
        }
    }
}

/// 认证令牌或TLS选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthTokenOrTlsOption {
    pub option_type: AuthTokenOrTlsOptionType,
    pub value: String,
}
impl AuthTokenOrTlsOption {
    pub fn restore(&self) -> String {
        if matches!(
            self.option_type,
            AuthTokenOrTlsOptionType::None
                | AuthTokenOrTlsOptionType::Ssl
                | AuthTokenOrTlsOptionType::X509
        ) {
            self.option_type.as_str().into()
        } else {
            format!(
                "{} {}",
                self.option_type.as_str(),
                quote_string(&self.value)
            )
        }
    }
}

/// 资源选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceOptionType {
    MaxQueriesPerHour,
    MaxUpdatesPerHour,
    MaxConnectionsPerHour,
    MaxUserConnections,
}
/// 资源选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceOption {
    pub option_type: ResourceOptionType,
    pub count: i64,
}
impl ResourceOption {
    pub fn restore(&self) -> String {
        let key = match self.option_type {
            ResourceOptionType::MaxQueriesPerHour => "MAX_QUERIES_PER_HOUR",
            ResourceOptionType::MaxUpdatesPerHour => "MAX_UPDATES_PER_HOUR",
            ResourceOptionType::MaxConnectionsPerHour => "MAX_CONNECTIONS_PER_HOUR",
            ResourceOptionType::MaxUserConnections => "MAX_USER_CONNECTIONS",
        };
        format!("{key} {}", self.count)
    }
}

/// 密码或锁选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordOrLockOptionType {
    PasswordExpire,
    PasswordExpireDefault,
    PasswordExpireNever,
    PasswordExpireInterval,
    PasswordHistory,
    PasswordHistoryDefault,
    PasswordReuseInterval,
    PasswordReuseDefault,
    Lock,
    Unlock,
    FailedLoginAttempts,
    PasswordLockTime,
    PasswordLockTimeUnbounded,
}
/// 密码或锁选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordOrLockOption {
    pub option_type: PasswordOrLockOptionType,
    pub count: i64,
}
impl PasswordOrLockOption {
    pub fn restore(&self) -> String {
        match self.option_type {
            PasswordOrLockOptionType::PasswordExpire => "PASSWORD EXPIRE".into(),
            PasswordOrLockOptionType::PasswordExpireDefault => "PASSWORD EXPIRE DEFAULT".into(),
            PasswordOrLockOptionType::PasswordExpireNever => "PASSWORD EXPIRE NEVER".into(),
            PasswordOrLockOptionType::PasswordExpireInterval => {
                format!("PASSWORD EXPIRE INTERVAL {} DAY", self.count)
            }
            PasswordOrLockOptionType::PasswordHistory => format!("PASSWORD HISTORY {}", self.count),
            PasswordOrLockOptionType::PasswordHistoryDefault => "PASSWORD HISTORY DEFAULT".into(),
            PasswordOrLockOptionType::PasswordReuseInterval => {
                format!("PASSWORD REUSE INTERVAL {} DAY", self.count)
            }
            PasswordOrLockOptionType::PasswordReuseDefault => {
                "PASSWORD REUSE INTERVAL DEFAULT".into()
            }
            PasswordOrLockOptionType::Lock => "ACCOUNT LOCK".into(),
            PasswordOrLockOptionType::Unlock => "ACCOUNT UNLOCK".into(),
            PasswordOrLockOptionType::FailedLoginAttempts => {
                format!("FAILED_LOGIN_ATTEMPTS {}", self.count)
            }
            PasswordOrLockOptionType::PasswordLockTime => {
                format!("PASSWORD_LOCK_TIME {}", self.count)
            }
            PasswordOrLockOptionType::PasswordLockTimeUnbounded => {
                "PASSWORD_LOCK_TIME UNBOUNDED".into()
            }
        }
    }
}

/// 用户元数据类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserMetadataType {
    Comment,
    Attribute,
}
/// 注释或属性选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentOrAttributeOption {
    pub option_type: UserMetadataType,
    pub value: String,
}
impl CommentOrAttributeOption {
    pub fn restore(&self) -> String {
        format!(
            " {} {}",
            match self.option_type {
                UserMetadataType::Comment => "COMMENT",
                UserMetadataType::Attribute => "ATTRIBUTE",
            },
            quote_string(&self.value)
        )
    }
}
/// 资源组名称选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupNameOption {
    pub value: String,
}
impl ResourceGroupNameOption {
    pub fn restore(&self) -> String {
        format!(" RESOURCE GROUP {}", quote_name(&self.value))
    }
}

/// 还原 account options 为 SQL 文本。
fn restore_account_options(
    specs: &[UserSpec],
    tls: &[AuthTokenOrTlsOption],
    resources: &[ResourceOption],
    passwords: &[PasswordOrLockOption],
    metadata: &Option<CommentOrAttributeOption>,
    group: &Option<ResourceGroupNameOption>,
) -> Result<String, String> {
    let mut sql = specs
        .iter()
        .map(UserSpec::restore)
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    if !tls.is_empty() {
        sql.push_str(" REQUIRE ");
        sql.push_str(
            &tls.iter()
                .map(AuthTokenOrTlsOption::restore)
                .collect::<Vec<_>>()
                .join(" AND "),
        );
    }
    if !resources.is_empty() {
        sql.push_str(" WITH ");
        sql.push_str(
            &resources
                .iter()
                .map(ResourceOption::restore)
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if !passwords.is_empty() {
        sql.push(' ');
        sql.push_str(
            &passwords
                .iter()
                .map(PasswordOrLockOption::restore)
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if let Some(value) = metadata {
        sql.push_str(&value.restore());
    }
    if let Some(value) = group {
        sql.push_str(&value.restore());
    }
    Ok(sql)
}

/// CREATE USER 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateUserStmt {
    pub is_create_role: bool,
    pub if_not_exists: bool,
    pub specs: Vec<UserSpec>,
    pub tls_options: Vec<AuthTokenOrTlsOption>,
    pub resource_options: Vec<ResourceOption>,
    pub password_options: Vec<PasswordOrLockOption>,
    pub metadata: Option<CommentOrAttributeOption>,
    pub resource_group: Option<ResourceGroupNameOption>,
}
impl CreateUserStmt {
    pub fn restore(&self) -> Result<String, String> {
        let prefix = if self.is_create_role {
            "CREATE ROLE "
        } else {
            "CREATE USER "
        };
        Ok(format!(
            "{prefix}{}{}",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            restore_account_options(
                &self.specs,
                &self.tls_options,
                &self.resource_options,
                &self.password_options,
                &self.metadata,
                &self.resource_group
            )?
        ))
    }
    pub fn secure_text(&self) -> String {
        format!(
            "create user{}",
            self.specs
                .iter()
                .map(|u| format!(" {}", u.security_string()))
                .collect::<String>()
        )
    }
}

/// ALTER USER 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterUserStmt {
    pub if_exists: bool,
    pub current_auth: Option<AuthOption>,
    pub current_dual_password: DualPasswordOptionType,
    pub specs: Vec<UserSpec>,
    pub tls_options: Vec<AuthTokenOrTlsOption>,
    pub resource_options: Vec<ResourceOption>,
    pub password_options: Vec<PasswordOrLockOption>,
    pub metadata: Option<CommentOrAttributeOption>,
    pub resource_group: Option<ResourceGroupNameOption>,
}
impl AlterUserStmt {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "ALTER USER {}",
            if self.if_exists { "IF EXISTS " } else { "" }
        );
        if let Some(auth) = &self.current_auth {
            sql.push_str("USER() ");
            sql.push_str(&auth.restore());
            if self.current_dual_password != DualPasswordOptionType::None {
                sql.push(' ');
                sql.push_str(self.current_dual_password.restore()?);
            }
        } else if self.current_dual_password != DualPasswordOptionType::None {
            sql.push_str("USER() ");
            sql.push_str(self.current_dual_password.restore()?);
        }
        sql.push_str(&restore_account_options(
            &self.specs,
            &self.tls_options,
            &self.resource_options,
            &self.password_options,
            &self.metadata,
            &self.resource_group,
        )?);
        Ok(sql)
    }
    pub fn secure_text(&self) -> String {
        format!(
            "alter user{}",
            self.specs
                .iter()
                .map(|u| format!(" {}", u.security_string()))
                .collect::<String>()
        )
    }
}

/// ALTER实例语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterInstanceStmt {
    pub reload_tls: bool,
    pub no_rollback_on_error: bool,
}
impl AlterInstanceStmt {
    pub fn restore(&self) -> String {
        format!(
            "ALTER INSTANCE{}{}",
            if self.reload_tls { " RELOAD TLS" } else { "" },
            if self.no_rollback_on_error {
                " NO ROLLBACK ON ERROR"
            } else {
                ""
            }
        )
    }
}
/// ALTER范围语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterRangeStmt {
    pub range_name: String,
    pub placement_option: String,
}
impl AlterRangeStmt {
    pub fn restore(&self) -> String {
        format!(
            "ALTER RANGE {} {}",
            quote_name(&self.range_name),
            self.placement_option
        )
    }
}
/// DROP用户语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropUserStmt {
    pub if_exists: bool,
    pub is_drop_role: bool,
    pub users: Vec<Identity>,
}
impl DropUserStmt {
    pub fn restore(&self) -> String {
        format!(
            "DROP {} {}{}",
            if self.is_drop_role { "ROLE" } else { "USER" },
            if self.if_exists { "IF EXISTS " } else { "" },
            self.users
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// 字符串或用户Var结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StringOrUserVar {
    pub string_literal: String,
    pub user_variable: Option<String>,
}
impl StringOrUserVar {
    pub fn restore(&self) -> String {
        if !self.string_literal.is_empty() {
            quote_string(&self.string_literal)
        } else {
            self.user_variable.clone().unwrap_or_default()
        }
    }
}
/// 推荐索引选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendIndexOption {
    pub option: String,
    pub value: String,
}
/// 推荐索引语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendIndexStmt {
    pub action: String,
    pub sql: String,
    pub id: i64,
    pub options: Vec<RecommendIndexOption>,
}
impl RecommendIndexStmt {
    pub fn restore(&self) -> String {
        let opts = self
            .options
            .iter()
            .map(|o| format!("{} = {}", o.option.to_uppercase(), o.value))
            .collect::<Vec<_>>()
            .join(", ");
        match self.action.as_str() {
            "run" => format!(
                "RECOMMEND INDEX RUN{}{}",
                if self.sql.is_empty() {
                    String::new()
                } else {
                    format!(" FOR {}", quote_string(&self.sql))
                },
                if opts.is_empty() {
                    String::new()
                } else {
                    format!(" WITH {opts}")
                }
            ),
            "show" => "RECOMMEND INDEX SHOW OPTION".into(),
            "apply" => format!("RECOMMEND INDEX APPLY {}", self.id),
            "ignore" => format!("RECOMMEND INDEX IGNORE {}", self.id),
            "set" => format!("RECOMMEND INDEX SET {opts}"),
            _ => "RECOMMEND INDEX".into(),
        }
    }
}

/// CREATE绑定语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateBindingStmt {
    pub global_scope: bool,
    pub origin: Option<String>,
    pub hinted: Option<String>,
    pub plan_digests: Vec<StringOrUserVar>,
}
impl CreateBindingStmt {
    pub fn restore(&self) -> String {
        let scope = if self.global_scope {
            "GLOBAL"
        } else {
            "SESSION"
        };
        if let Some(origin) = &self.origin {
            format!(
                "CREATE {scope} BINDING FOR {origin} USING {}",
                self.hinted.as_deref().unwrap_or_default()
            )
        } else {
            format!(
                "CREATE {scope} BINDING FROM HISTORY USING PLAN DIGEST {}",
                self.plan_digests
                    .iter()
                    .map(StringOrUserVar::restore)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}
/// DROP绑定语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropBindingStmt {
    pub global_scope: bool,
    pub origin: Option<String>,
    pub hinted: Option<String>,
    pub sql_digests: Vec<StringOrUserVar>,
}
impl DropBindingStmt {
    pub fn restore(&self) -> String {
        let scope = if self.global_scope {
            "GLOBAL"
        } else {
            "SESSION"
        };
        let target = if let Some(origin) = &self.origin {
            format!(
                "{origin}{}",
                self.hinted
                    .as_ref()
                    .map(|h| format!(" USING {h}"))
                    .unwrap_or_default()
            )
        } else {
            format!(
                "SQL DIGEST {}",
                self.sql_digests
                    .iter()
                    .map(StringOrUserVar::restore)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!("DROP {scope} BINDING FOR {target}")
    }
}
/// 绑定状态类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BindingStatusType {
    #[default]
    Enabled,
    Disabled,
}
/// SET绑定语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetBindingStmt {
    pub status: BindingStatusType,
    pub origin: Option<String>,
    pub hinted: Option<String>,
    pub sql_digest: String,
}
impl SetBindingStmt {
    pub fn restore(&self) -> String {
        let target = if let Some(origin) = &self.origin {
            format!(
                "{origin}{}",
                self.hinted
                    .as_ref()
                    .map(|h| format!(" USING {h}"))
                    .unwrap_or_default()
            )
        } else {
            format!("SQL DIGEST {}", quote_string(&self.sql_digest))
        };
        format!(
            "SET BINDING {} FOR {target}",
            if self.status == BindingStatusType::Enabled {
                "ENABLED"
            } else {
                "DISABLED"
            }
        )
    }
}

/// 统计信息类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatisticsType {
    #[default]
    Cardinality,
    Dependency,
    Correlation,
}
impl StatisticsType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cardinality => "cardinality",
            Self::Dependency => "dependency",
            Self::Correlation => "correlation",
        }
    }
}
/// 统计信息规格结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatisticsSpec {
    pub name: String,
    pub statistics_type: StatisticsType,
    pub columns: Vec<String>,
}
/// CREATE统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateStatisticsStmt {
    pub if_not_exists: bool,
    pub name: String,
    pub statistics_type: StatisticsType,
    pub table: String,
    pub columns: Vec<String>,
}
impl CreateStatisticsStmt {
    pub fn restore(&self) -> String {
        format!(
            "CREATE STATISTICS {}{} ({}) ON {}({})",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            quote_name(&self.name),
            self.statistics_type.as_str(),
            self.table,
            self.columns
                .iter()
                .map(|c| quote_name(c))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
/// DROP统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropStatisticsStmt {
    pub name: String,
}
impl DropStatisticsStmt {
    pub fn restore(&self) -> String {
        format!("DROP STATISTICS {}", quote_name(&self.name))
    }
}
/// DO 语句节点：求值表达式但不返回结果集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoStmt {
    pub expressions: Vec<String>,
}
impl DoStmt {
    pub fn restore(&self) -> String {
        format!("DO {}", self.expressions.join(", "))
    }
}

/// ADMIN语句类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminStmtType {
    ShowDdl,
    CheckTable,
    ShowDdlJobs,
    CancelDdlJobs,
    PauseDdlJobs,
    ResumeDdlJobs,
    CheckIndex,
    RecoverIndex,
    CleanupIndex,
    CheckIndexRange,
    ShowDdlJobQueries,
    ShowDdlJobQueriesWithRange,
    ChecksumTable,
    ShowSlow,
    ShowNextRowId,
    ReloadExprPushdownBlacklist,
    ReloadOptRuleBlacklist,
    PluginDisable,
    PluginEnable,
    FlushBindings,
    CaptureBindings,
    EvolveBindings,
    ReloadBindings,
    ReloadStatistics,
    FlushPlanCache,
    SetBdrRole,
    ShowBdrRole,
    UnsetBdrRole,
    AlterDdlJob,
    WorkloadRepoCreate,
    ReloadClusterBindings,
}
/// 语句作用域枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatementScope {
    #[default]
    None,
    Session,
    Instance,
    Global,
}
/// Bdr角色枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BdrRole {
    #[default]
    None,
    Primary,
    Secondary,
    /// A role unknown to this binary. Go's string-backed BDRRole permits
    /// unknown values and the DDL guards treat them like an unset role.
    Unknown,
}
/// SHOW慢查询类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowSlowType {
    #[default]
    Top,
    Recent,
}
/// SHOW慢查询种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowSlowKind {
    #[default]
    Default,
    Internal,
    All,
}
/// SHOW慢查询结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowSlow {
    pub show_type: ShowSlowType,
    pub count: u64,
    pub kind: ShowSlowKind,
}
impl ShowSlow {
    pub fn restore(&self) -> String {
        match self.show_type {
            ShowSlowType::Recent => format!("RECENT {}", self.count),
            ShowSlowType::Top => format!(
                "TOP {}{}",
                match self.kind {
                    ShowSlowKind::Default => "",
                    ShowSlowKind::Internal => "INTERNAL ",
                    ShowSlowKind::All => "ALL ",
                },
                self.count
            ),
        }
    }
}
/// 句柄范围结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HandleRange {
    pub begin: i64,
    pub end: i64,
}
/// LIMIT简单结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LimitSimple {
    pub count: u64,
    pub offset: u64,
}
/// ALTER任务选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterJobOption {
    pub name: String,
    pub value: Option<String>,
}
impl AlterJobOption {
    pub fn restore(&self) -> String {
        self.value
            .as_ref()
            .map(|v| format!("{} = {v}", self.name))
            .unwrap_or_else(|| self.name.clone())
    }
}
/// ADMIN 诊断/维护语句 AST。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminStmt {
    pub statement_type: AdminStmtType,
    pub index: String,
    pub tables: Vec<String>,
    pub job_ids: Vec<i64>,
    pub job_number: i64,
    pub handle_ranges: Vec<HandleRange>,
    pub show_slow: Option<ShowSlow>,
    pub plugins: Vec<String>,
    pub where_clause: Option<String>,
    pub scope: StatementScope,
    pub limit: LimitSimple,
    pub bdr_role: BdrRole,
    pub alter_job_options: Vec<AlterJobOption>,
}
impl AdminStmt {
    pub fn restore(&self) -> Result<String, String> {
        let tables = self.tables.join(", ");
        let jobs = self
            .job_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let body = match self.statement_type {
            AdminStmtType::ShowDdl => "SHOW DDL".into(),
            AdminStmtType::CheckTable => format!("CHECK TABLE {tables}"),
            AdminStmtType::ShowDdlJobs => format!(
                "SHOW DDL JOBS{}{}",
                if self.job_number == 0 {
                    String::new()
                } else {
                    format!(" {}", self.job_number)
                },
                self.where_clause
                    .as_ref()
                    .map(|w| format!(" WHERE {w}"))
                    .unwrap_or_default()
            ),
            AdminStmtType::CancelDdlJobs => format!("CANCEL DDL JOBS {jobs}"),
            AdminStmtType::PauseDdlJobs => format!("PAUSE DDL JOBS {jobs}"),
            AdminStmtType::ResumeDdlJobs => format!("RESUME DDL JOBS {jobs}"),
            AdminStmtType::CheckIndex
            | AdminStmtType::RecoverIndex
            | AdminStmtType::CleanupIndex => format!(
                "{} INDEX {tables} {}",
                match self.statement_type {
                    AdminStmtType::CheckIndex => "CHECK",
                    AdminStmtType::RecoverIndex => "RECOVER",
                    _ => "CLEANUP",
                },
                self.index
            ),
            AdminStmtType::CheckIndexRange => format!(
                "CHECK INDEX {tables} {} {}",
                self.index,
                self.handle_ranges
                    .iter()
                    .map(|r| format!("({},{})", r.begin, r.end))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AdminStmtType::ShowDdlJobQueries => format!("SHOW DDL JOB QUERIES {jobs}"),
            AdminStmtType::ShowDdlJobQueriesWithRange => {
                format!(
                    "SHOW DDL JOB QUERIES LIMIT {}, {}",
                    self.limit.offset, self.limit.count
                )
            }
            AdminStmtType::ChecksumTable => format!("CHECKSUM TABLE {tables}"),
            AdminStmtType::ShowSlow => format!(
                "SHOW SLOW {}",
                self.show_slow.as_ref().ok_or("missing ShowSlow")?.restore()
            ),
            AdminStmtType::ShowNextRowId => format!("SHOW {tables} NEXT_ROW_ID"),
            AdminStmtType::ReloadExprPushdownBlacklist => "RELOAD EXPR_PUSHDOWN_BLACKLIST".into(),
            AdminStmtType::ReloadOptRuleBlacklist => "RELOAD OPT_RULE_BLACKLIST".into(),
            AdminStmtType::PluginDisable | AdminStmtType::PluginEnable => format!(
                "PLUGINS {} {}",
                if self.statement_type == AdminStmtType::PluginEnable {
                    "ENABLE"
                } else {
                    "DISABLE"
                },
                self.plugins.join(", ")
            ),
            AdminStmtType::FlushBindings => "FLUSH BINDINGS".into(),
            AdminStmtType::CaptureBindings => "CAPTURE BINDINGS".into(),
            AdminStmtType::EvolveBindings => "EVOLVE BINDINGS".into(),
            AdminStmtType::ReloadBindings => "RELOAD BINDINGS".into(),
            AdminStmtType::ReloadClusterBindings => "RELOAD CLUSTER BINDINGS".into(),
            AdminStmtType::ReloadStatistics => "RELOAD STATS_EXTENDED".into(),
            AdminStmtType::FlushPlanCache => format!(
                "FLUSH {} PLAN_CACHE",
                match self.scope {
                    StatementScope::Session => "SESSION",
                    StatementScope::Instance => "INSTANCE",
                    StatementScope::Global => "GLOBAL",
                    StatementScope::None => return Err("missing statement scope".into()),
                }
            ),
            AdminStmtType::SetBdrRole => format!(
                "SET BDR ROLE {}",
                match self.bdr_role {
                    BdrRole::Primary => "PRIMARY",
                    BdrRole::Secondary => "SECONDARY",
                    BdrRole::None | BdrRole::Unknown => {
                        return Err("Unsupported BDR role".into());
                    }
                }
            ),
            AdminStmtType::ShowBdrRole => "SHOW BDR ROLE".into(),
            AdminStmtType::UnsetBdrRole => "UNSET BDR ROLE".into(),
            AdminStmtType::AlterDdlJob => format!(
                "ALTER DDL JOBS {} {}",
                self.job_number,
                self.alter_job_options
                    .iter()
                    .map(AlterJobOption::restore)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AdminStmtType::WorkloadRepoCreate => "CREATE WORKLOAD SNAPSHOT".into(),
        };
        Ok(format!("ADMIN {body}"))
    }
}

/// 认证令牌或TLS选项类型别名。
pub type AuthTokenOrTLSOption = AuthTokenOrTlsOption;
/// 认证令牌或TLS选项类型类型别名。
pub type AuthTokenOrTLSOptionType = AuthTokenOrTlsOptionType;
/// BDR角色类型别名。
pub type BDRRole = BdrRole;

/// 流量操作类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrafficOpType {
    #[default]
    Capture,
    Replay,
    Show,
    Cancel,
}
/// 流量选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrafficOptionType {
    Duration,
    EncryptionMethod,
    Compress,
    Username,
    Password,
    Speed,
    ReadOnly,
}
/// 流量选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrafficOption {
    pub option_type: TrafficOptionType,
    pub string_value: String,
    pub uint_value: u64,
    pub float_value: String,
    pub bool_value: bool,
}
/// 流量捕获/回放语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrafficStmt {
    pub operation: TrafficOpType,
    pub options: Vec<TrafficOption>,
    pub directory: String,
}
impl TrafficStmt {
    pub fn restore(&self) -> String {
        if self.operation == TrafficOpType::Show {
            return "SHOW TRAFFIC JOBS".into();
        }
        if self.operation == TrafficOpType::Cancel {
            return "CANCEL TRAFFIC JOBS".into();
        }
        let mut sql = format!(
            "TRAFFIC {} {}",
            if self.operation == TrafficOpType::Capture {
                "CAPTURE TO"
            } else {
                "REPLAY FROM"
            },
            quote_string(&self.directory)
        );
        for o in &self.options {
            sql.push(' ');
            let rendered = match o.option_type {
                TrafficOptionType::Duration => {
                    format!("DURATION = {}", quote_string(&o.string_value))
                }
                TrafficOptionType::EncryptionMethod => {
                    format!("ENCRYPTION_METHOD = {}", quote_string(&o.string_value))
                }
                TrafficOptionType::Compress => {
                    if o.bool_value {
                        "COMPRESS = TRUE".into()
                    } else {
                        "COMPRESS = FALSE".into()
                    }
                }
                TrafficOptionType::Username => {
                    format!("USER = {}", quote_string(&o.string_value))
                }
                TrafficOptionType::Password => {
                    format!("PASSWORD = {}", quote_string(&o.string_value))
                }
                TrafficOptionType::Speed => format!("SPEED = {}", o.float_value),
                TrafficOptionType::ReadOnly => {
                    if o.bool_value {
                        "READONLY = TRUE".into()
                    } else {
                        "READONLY = FALSE".into()
                    }
                }
            };
            sql.push_str(&rendered);
        }
        sql
    }
    pub fn secure_text(&self) -> String {
        let mut copy = self.clone();
        if matches!(
            copy.operation,
            TrafficOpType::Capture | TrafficOpType::Replay
        ) {
            copy.directory = redact_url(&copy.directory);
        }
        for o in &mut copy.options {
            if matches!(o.option_type, TrafficOptionType::Password) {
                o.string_value = "xxxxxx".into();
            }
        }
        copy.restore()
    }
}

/// 压缩副本种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompactReplicaKind {
    #[default]
    All,
    TiFlash,
    TiKv,
}
/// 压缩表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactTableStmt {
    pub table: String,
    pub partitions: Vec<String>,
    pub replica_kind: CompactReplicaKind,
}
impl CompactTableStmt {
    pub fn restore(&self) -> String {
        let mut sql = format!("ALTER TABLE {} COMPACT", self.table);
        if !self.partitions.is_empty() {
            sql.push_str(" PARTITION ");
            sql.push_str(
                &self
                    .partitions
                    .iter()
                    .map(|p| quote_name(p))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        match self.replica_kind {
            CompactReplicaKind::TiFlash => sql.push_str(" TIFLASH REPLICA"),
            CompactReplicaKind::TiKv => sql.push_str(" TIKV REPLICA"),
            CompactReplicaKind::All => {}
        }
        sql
    }
}

/// FLUSH语句类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushStmtType {
    None,
    Tables,
    Privileges,
    Status,
    TiDbPlugin,
    Hosts,
    Logs,
    ClientErrorsSummary,
    StatsDelta,
}
/// Log类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogType {
    #[default]
    Default,
    Binary,
    Engine,
    Error,
    General,
    Slow,
}
/// FLUSH语句结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlushStmt {
    pub statement_type: FlushStmtType,
    pub no_write_to_binlog: bool,
    pub log_type: LogType,
    pub tables: Vec<String>,
    pub read_lock: bool,
    pub plugins: Vec<String>,
    pub is_cluster: bool,
    pub flush_objects: Vec<String>,
}
impl FlushStmt {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "FLUSH {}",
            if self.no_write_to_binlog {
                "NO_WRITE_TO_BINLOG "
            } else {
                ""
            }
        );
        let body = match self.statement_type {
            FlushStmtType::Tables => format!(
                "TABLES{}{}",
                if self.tables.is_empty() {
                    String::new()
                } else {
                    format!(" {}", self.tables.join(", "))
                },
                if self.read_lock {
                    " WITH READ LOCK"
                } else {
                    ""
                }
            ),
            FlushStmtType::Privileges => "PRIVILEGES".into(),
            FlushStmtType::Status => "STATUS".into(),
            FlushStmtType::TiDbPlugin => format!("TIDB PLUGINS {}", self.plugins.join(", ")),
            FlushStmtType::Hosts => "HOSTS".into(),
            FlushStmtType::Logs => match self.log_type {
                LogType::Default => "LOGS",
                LogType::Binary => "BINARY LOGS",
                LogType::Engine => "ENGINE LOGS",
                LogType::Error => "ERROR LOGS",
                LogType::General => "GENERAL LOGS",
                LogType::Slow => "SLOW LOGS",
            }
            .into(),
            FlushStmtType::ClientErrorsSummary => "CLIENT_ERRORS_SUMMARY".into(),
            FlushStmtType::StatsDelta => format!(
                "STATS_DELTA {}{}",
                self.flush_objects.join(", "),
                if self.is_cluster { " CLUSTER" } else { "" }
            ),
            FlushStmtType::None => return Err("Unsupported type of FlushStmt".into()),
        };
        sql.push_str(&body);
        Ok(sql)
    }
}

/// KILL语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KillStmt {
    pub query: bool,
    pub connection_id: u64,
    pub tidb_extension: bool,
    pub expression: Option<String>,
}
impl KillStmt {
    pub fn restore(&self) -> String {
        format!(
            "KILL{}{} {}",
            if self.tidb_extension { " TIDB" } else { "" },
            if self.query { " QUERY" } else { "" },
            self.expression
                .clone()
                .unwrap_or_else(|| self.connection_id.to_string())
        )
    }
}

/// TRACE语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceStmt {
    pub statement: String,
    pub format: String,
    pub trace_plan: bool,
    pub trace_plan_target: String,
}
impl TraceStmt {
    pub fn restore(&self) -> String {
        if self.trace_plan {
            format!(
                "TRACE PLAN {}{}",
                if self.trace_plan_target.is_empty() {
                    String::new()
                } else {
                    format!("TARGET = {} ", quote_string(&self.trace_plan_target))
                },
                self.statement
            )
        } else if self.format != "row" {
            format!(
                "TRACE FORMAT = {} {}",
                quote_string(&self.format),
                self.statement
            )
        } else {
            format!("TRACE {}", self.statement)
        }
    }
}
/// EXPLAINFOR语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplainForStmt {
    pub format: String,
    pub connection_id: u64,
}
impl ExplainForStmt {
    pub fn restore(&self) -> String {
        format!(
            "EXPLAIN FORMAT = {} FOR CONNECTION {}",
            quote_string(&self.format),
            self.connection_id
        )
    }
}
/// EXPLAIN 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplainStmt {
    pub statement: Option<String>,
    pub format: String,
    pub analyze: bool,
    pub explore: bool,
    pub sql_digest: String,
    pub replayer_file: String,
    pub plan_digest: String,
}
impl ExplainStmt {
    pub fn restore(&self) -> String {
        let mut sql = String::from("EXPLAIN ");
        if self.analyze {
            sql.push_str("ANALYZE ");
        }
        if self.explore {
            sql.push_str("EXPLORE ");
            if !self.replayer_file.is_empty() {
                sql.push_str("REPLAYER ");
                sql.push_str(&quote_string(&self.replayer_file));
            } else if !self.sql_digest.is_empty() {
                sql.push_str(&quote_string(&self.sql_digest));
            }
        } else if !self.analyze || self.format.to_lowercase() != "row" {
            sql.push_str(&format!("FORMAT = {} ", quote_string(&self.format)));
        }
        if !self.plan_digest.is_empty() {
            sql.push_str(&quote_string(&self.plan_digest));
        }
        if let Some(stmt) = &self.statement {
            sql.push_str(stmt);
        }
        sql
    }
}

/// SET字符集语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetCharsetStmt {
    pub charset: String,
    pub collate: String,
}
/// SHUTDOWN语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShutdownStmt;
impl ShutdownStmt {
    pub fn restore(&self) -> &'static str {
        "SHUTDOWN"
    }
}
/// RESTART语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestartStmt;
impl RestartStmt {
    pub fn restore(&self) -> &'static str {
        "RESTART"
    }
}
/// HELP语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HelpStmt {
    pub topic: String,
}
impl HelpStmt {
    pub fn restore(&self) -> String {
        format!("HELP {}", quote_string(&self.topic))
    }
}
/// 用户To用户结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserToUser {
    pub old_user: Identity,
    pub new_user: Identity,
}
impl UserToUser {
    pub fn restore(&self) -> String {
        format!("{} TO {}", self.old_user.restore(), self.new_user.restore())
    }
}
/// 重命名用户语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenameUserStmt {
    pub mappings: Vec<UserToUser>,
}
impl RenameUserStmt {
    pub fn restore(&self) -> String {
        format!(
            "RENAME USER {}",
            self.mappings
                .iter()
                .map(UserToUser::restore)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// 权限元素结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrivElem {
    pub privilege: String,
    pub columns: Vec<String>,
    pub name: String,
}
impl PrivElem {
    pub fn restore(&self) -> Result<String, String> {
        let key = if !self.name.is_empty() {
            self.name.to_uppercase()
        } else if !self.privilege.is_empty() {
            self.privilege.to_uppercase()
        } else {
            return Err("Undefined privilege type".into());
        };
        Ok(if self.columns.is_empty() {
            key
        } else {
            format!(
                "{key} ({})",
                self.columns
                    .iter()
                    .map(|c| quote_name(c))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
    }
}
/// 角色或权限结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoleOrPriv {
    pub symbols: String,
    pub role: Option<Identity>,
    pub privilege: Option<PrivElem>,
}
impl RoleOrPriv {
    pub fn to_role(&self) -> Result<Identity, String> {
        if self.privilege.is_some() {
            return Err("can't convert to RoleIdentity".into());
        }
        Ok(self
            .role
            .clone()
            .unwrap_or_else(|| Identity::new(&self.symbols, "%")))
    }
    pub fn to_priv(&self) -> Result<PrivElem, String> {
        if self.role.is_some() {
            return Err("can't convert to PrivElem".into());
        }
        if self.symbols.is_empty() && self.privilege.is_none() {
            return Err("symbols should not be length 0".into());
        }
        Ok(self.privilege.clone().unwrap_or_else(|| PrivElem {
            name: self.symbols.clone(),
            ..PrivElem::default()
        }))
    }
}
/// 对象类型类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObjectTypeType {
    #[default]
    None,
    Table,
    Function,
    Procedure,
}
impl ObjectTypeType {
    pub fn restore(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Table => "TABLE",
            Self::Function => "FUNCTION",
            Self::Procedure => "PROCEDURE",
        }
    }
}
/// GRANT级别类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GrantLevelType {
    #[default]
    None,
    Global,
    Database,
    Table,
}
/// GRANT级别结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantLevel {
    pub level: GrantLevelType,
    pub db_name: String,
    pub table_name: String,
}
impl GrantLevel {
    pub fn restore(&self) -> String {
        match self.level {
            GrantLevelType::Global => "*.*".into(),
            GrantLevelType::Database => {
                if self.db_name.is_empty() {
                    "*".into()
                } else {
                    format!("{}.*", quote_name(&self.db_name))
                }
            }
            GrantLevelType::Table => {
                if self.db_name.is_empty() {
                    quote_name(&self.table_name)
                } else {
                    format!(
                        "{}.{}",
                        quote_name(&self.db_name),
                        quote_name(&self.table_name)
                    )
                }
            }
            GrantLevelType::None => String::new(),
        }
    }
}
/// 还原 privileges 为 SQL 文本。
fn restore_privileges(values: &[PrivElem]) -> Result<String, String> {
    values
        .iter()
        .map(PrivElem::restore)
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join(", "))
}
/// REVOKE 权限语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevokeStmt {
    pub privileges: Vec<PrivElem>,
    pub object_type: ObjectTypeType,
    pub level: GrantLevel,
    pub users: Vec<UserSpec>,
}
impl RevokeStmt {
    pub fn restore(&self) -> Result<String, String> {
        Ok(format!(
            "REVOKE {} ON {}{} FROM {}",
            restore_privileges(&self.privileges)?,
            if self.object_type == ObjectTypeType::None {
                String::new()
            } else {
                format!("{} ", self.object_type.restore())
            },
            self.level.restore(),
            self.users
                .iter()
                .map(UserSpec::restore)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        ))
    }
}
/// REVOKE角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevokeRoleStmt {
    pub roles: Vec<Identity>,
    pub users: Vec<Identity>,
}
impl RevokeRoleStmt {
    pub fn restore(&self) -> String {
        format!(
            "REVOKE {} FROM {}",
            self.roles
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", "),
            self.users
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
/// GRANT 权限语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantStmt {
    pub privileges: Vec<PrivElem>,
    pub object_type: ObjectTypeType,
    pub level: GrantLevel,
    pub users: Vec<UserSpec>,
    pub tls_options: Vec<AuthTokenOrTlsOption>,
    pub with_grant: bool,
    pub original_text: String,
}
impl GrantStmt {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "GRANT {} ON {}{} TO {}",
            restore_privileges(&self.privileges)?,
            if self.object_type == ObjectTypeType::None {
                String::new()
            } else {
                format!("{} ", self.object_type.restore())
            },
            self.level.restore(),
            self.users
                .iter()
                .map(UserSpec::restore)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        );
        if !self.tls_options.is_empty() {
            sql.push_str(" REQUIRE ");
            sql.push_str(
                &self
                    .tls_options
                    .iter()
                    .map(AuthTokenOrTlsOption::restore)
                    .collect::<Vec<_>>()
                    .join(" AND "),
            );
        }
        if self.with_grant {
            sql.push_str(" WITH GRANT OPTION");
        }
        Ok(sql)
    }
    pub fn secure_text(&self) -> String {
        self.original_text
            .to_lowercase()
            .find("identified")
            .map(|i| self.original_text[..i].to_owned())
            .unwrap_or_else(|| self.original_text.clone())
    }
}
/// GRANT代理语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantProxyStmt {
    pub local_user: Identity,
    pub external_users: Vec<Identity>,
    pub with_grant: bool,
}
impl GrantProxyStmt {
    pub fn restore(&self) -> String {
        format!(
            "GRANT PROXY ON {} TO {}{}",
            self.local_user.restore(),
            self.external_users
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", "),
            if self.with_grant {
                " WITH GRANT OPTION"
            } else {
                ""
            }
        )
    }
}
/// GRANT角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantRoleStmt {
    pub roles: Vec<Identity>,
    pub users: Vec<Identity>,
    pub original_text: String,
}
impl GrantRoleStmt {
    pub fn restore(&self) -> String {
        format!(
            "GRANT {} TO {}",
            self.roles
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", "),
            self.users
                .iter()
                .map(Identity::restore)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
    pub fn secure_text(&self) -> String {
        self.original_text
            .to_lowercase()
            .find("identified")
            .map(|i| self.original_text[..i].to_owned())
            .unwrap_or_else(|| self.original_text.clone())
    }
}

/// BRIE 操作种类（Backup/Restore/Import/Export）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BrieKind {
    #[default]
    Backup,
    CancelJob,
    StreamStart,
    StreamMetadata,
    StreamStatus,
    StreamPause,
    StreamResume,
    StreamStop,
    StreamPurge,
    Restore,
    RestorePoint,
    ShowJob,
    ShowQuery,
    ShowBackupMetadata,
}
/// 备份恢复导入导出种类类型别名。
pub type BRIEKind = BrieKind;
impl BrieKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Backup => "BACKUP",
            Self::CancelJob => "CANCEL BR JOB",
            Self::StreamStart => "BACKUP LOGS",
            Self::StreamMetadata => "SHOW BACKUP LOGS METADATA",
            Self::StreamStatus => "SHOW BACKUP LOGS STATUS",
            Self::StreamPause => "PAUSE BACKUP LOGS",
            Self::StreamResume => "RESUME BACKUP LOGS",
            Self::StreamStop => "STOP BACKUP LOGS",
            Self::StreamPurge => "PURGE BACKUP LOGS",
            Self::Restore => "RESTORE",
            Self::RestorePoint => "RESTORE POINT",
            Self::ShowJob => "SHOW BR JOB",
            Self::ShowQuery => "SHOW BR JOB QUERY",
            Self::ShowBackupMetadata => "SHOW BACKUP METADATA",
        }
    }
}
/// 备份恢复导入导出选项级别枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BrieOptionLevel {
    #[default]
    Off,
    Required,
    Optional,
}
/// 备份恢复导入导出选项级别类型别名。
pub type BRIEOptionLevel = BrieOptionLevel;
impl BrieOptionLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Required => "REQUIRED",
            Self::Optional => "OPTIONAL",
        }
    }
}
/// 备份恢复导入导出选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BRIEOption {
    pub option_type: String,
    pub string_value: String,
    pub uint_value: u64,
    pub level: Option<BrieOptionLevel>,
}
impl BRIEOption {
    pub fn restore(&self) -> String {
        let value = if let Some(level) = self.level {
            level.as_str().into()
        } else if !self.string_value.is_empty() {
            quote_string(&self.string_value)
        } else if self.option_type == "RATE_LIMIT" {
            format!("{} MB/SECOND", self.uint_value / 1_048_576)
        } else {
            self.uint_value.to_string()
        };
        format!("{} = {value}", self.option_type.to_uppercase())
    }
}
/// 备份/恢复/导入/导出（BRIE）语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BRIEStmt {
    pub kind: BrieKind,
    pub schemas: Vec<String>,
    pub tables: Vec<String>,
    pub storage: String,
    pub job_id: i64,
    pub options: Vec<BRIEOption>,
}
impl BRIEStmt {
    pub fn restore(&self) -> String {
        let mut sql = self.kind.as_str().to_owned();
        match self.kind {
            BrieKind::Backup | BrieKind::Restore => {
                if !self.tables.is_empty() {
                    sql.push_str(" TABLE ");
                    sql.push_str(&self.tables.join(", "));
                } else if !self.schemas.is_empty() {
                    sql.push_str(" DATABASE ");
                    sql.push_str(
                        &self
                            .schemas
                            .iter()
                            .map(|s| quote_name(s))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                } else {
                    sql.push_str(" DATABASE *");
                }
                sql.push_str(if self.kind == BrieKind::Backup {
                    " TO "
                } else {
                    " FROM "
                });
                sql.push_str(&quote_string(&self.storage));
            }
            BrieKind::CancelJob | BrieKind::ShowJob | BrieKind::ShowQuery => {
                sql.push_str(&format!(" {}", self.job_id))
            }
            BrieKind::StreamStart => {
                sql.push_str(" TO ");
                sql.push_str(&quote_string(&self.storage));
            }
            BrieKind::RestorePoint
            | BrieKind::StreamMetadata
            | BrieKind::ShowBackupMetadata
            | BrieKind::StreamPurge => {
                sql.push_str(" FROM ");
                sql.push_str(&quote_string(&self.storage));
            }
            _ => {}
        }
        for option in &self.options {
            sql.push(' ');
            sql.push_str(&option.restore());
        }
        sql
    }
    pub fn secure_text(&self) -> String {
        let mut copy = self.clone();
        copy.storage = redact_url(&copy.storage);
        copy.restore()
    }
}

/// IMPORTINTO动作类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImportIntoActionTp {
    #[default]
    Cancel,
}
/// IMPORTINTO动作语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportIntoActionStmt {
    pub action: ImportIntoActionTp,
    pub job_id: i64,
}
impl ImportIntoActionStmt {
    pub fn restore(&self) -> String {
        format!("CANCEL IMPORT JOB {}", self.job_id)
    }
}
/// 取消分布任务语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CancelDistributionJobStmt {
    pub job_id: i64,
}
impl CancelDistributionJobStmt {
    pub fn restore(&self) -> String {
        format!("CANCEL DISTRIBUTION JOB {}", self.job_id)
    }
}

/// 标识符结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ident {
    pub schema: String,
    pub name: String,
}
impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.schema.is_empty() {
            f.write_str(&self.name)
        } else {
            write!(f, "{}.{}", self.schema, self.name)
        }
    }
}
/// SELECT语句Opts结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectStmtOpts {
    pub distinct: bool,
    pub sql_big_result: bool,
    pub sql_buffer_result: bool,
    pub sql_cache: bool,
    pub sql_small_result: bool,
    pub calc_found_rows: bool,
    pub straight_join: bool,
    pub priority: i32,
    pub table_hints: Vec<TableOptimizerHint>,
    pub explicit_all: bool,
}
/// 优化器提示时间范围结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintTimeRange {
    pub from: String,
    pub to: String,
}
/// 优化器提示SETVar结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintSetVar {
    pub variable_name: String,
    pub value: String,
}
/// 优化器提示载荷：数值、布尔、名称、LEADING 列表等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HintData {
    None,
    Unsigned(u64),
    Signed(i64),
    Boolean(bool),
    Name(String),
    TimeRange(HintTimeRange),
    SetVar(HintSetVar),
    Leading(LeadingList),
}
impl Default for HintData {
    fn default() -> Self {
        Self::None
    }
}
/// 表级优化器提示（optimizer hint）节点。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableOptimizerHint {
    pub hint_name: String,
    pub data: HintData,
    pub qb_name: String,
    pub tables: Vec<HintTable>,
    pub indexes: Vec<String>,
}
impl TableOptimizerHint {
    pub fn restore(&self) -> Result<String, String> {
        let name = self.hint_name.to_uppercase();
        let lower = self.hint_name.to_lowercase();
        let qb = if self.qb_name.is_empty() {
            String::new()
        } else if lower == "qb_name" {
            quote_name(&self.qb_name)
        } else {
            format!("@{}", quote_name(&self.qb_name))
        };
        let tables = self
            .tables
            .iter()
            .map(HintTable::restore)
            .collect::<Vec<_>>();
        let payload = match (&*lower, &self.data) {
            ("leading", HintData::Leading(list)) => list.restore_with_qb(
                None,
                self.tables.first().is_some_and(|t| !t.qb_name.is_empty()),
            ),
            ("max_execution_time", HintData::Unsigned(v)) => v.to_string(),
            ("nth_plan", HintData::Signed(v)) => v.to_string(),
            ("memory_quota", HintData::Signed(v)) => format!("{} MB", v / 1024 / 1024),
            ("use_toja" | "use_cascades", HintData::Boolean(v)) => {
                if *v {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
            ("time_range", HintData::TimeRange(v)) => {
                format!("{}, {}", quote_string(&v.from), quote_string(&v.to))
            }
            ("set_var", HintData::SetVar(v)) => {
                format!("{} = {}", v.variable_name, quote_string(&v.value))
            }
            ("query_type" | "resource_group", HintData::Name(v)) => v.to_uppercase(),
            ("read_from_storage", HintData::Name(v)) => {
                let mut value = v.to_uppercase();
                if !tables.is_empty() {
                    value.push('[');
                    value.push_str(&tables.join(", "));
                    value.push(']');
                }
                value
            }
            ("qb_name", _) if !tables.is_empty() => tables.join(". "),
            (
                "use_index"
                | "ignore_index"
                | "use_index_merge"
                | "force_index"
                | "order_index"
                | "no_order_index"
                | "index_lookup_pushdown"
                | "no_index_lookup_pushdown",
                _,
            ) => {
                let table = tables.first().ok_or("index hint requires a table")?;
                format!(
                    "{table} {}",
                    self.indexes
                        .iter()
                        .map(|i| quote_name(i))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            (_, _) if !tables.is_empty() => tables.join(", "),
            (_, HintData::None) => String::new(),
            _ => return Err(format!("unsupported hint data for {}", self.hint_name)),
        };
        let mut args = qb;
        if !args.is_empty() && !payload.is_empty() {
            args.push_str(if lower == "qb_name" { ", " } else { " " });
        }
        args.push_str(&payload);
        Ok(format!("{name}({args})"))
    }
}

/// 文本字符串结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextString {
    pub value: String,
    pub is_binary_literal: bool,
}
/// 二进制字面量特质接口。
pub trait BinaryLiteral {
    fn to_string_value(&self) -> String;
}
/// SET资源组语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetResourceGroupStmt {
    pub name: String,
}
impl SetResourceGroupStmt {
    pub fn restore(&self) -> String {
        format!("SET RESOURCE GROUP {}", quote_name(&self.name))
    }
}

/// 校准资源类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CalibrateResourceType {
    #[default]
    None,
    Tpcc,
    OltpReadWrite,
    OltpReadOnly,
    OltpWriteOnly,
    Tpch10,
}
impl CalibrateResourceType {
    pub fn restore(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Tpcc => " WORKLOAD TPCC",
            Self::OltpReadWrite => " WORKLOAD OLTP_READ_WRITE",
            Self::OltpReadOnly => " WORKLOAD OLTP_READ_ONLY",
            Self::OltpWriteOnly => " WORKLOAD OLTP_WRITE_ONLY",
            Self::Tpch10 => " WORKLOAD TPCH_10",
        }
    }
}
/// 动态校准类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DynamicCalibrateType {
    StartTime,
    EndTime,
    Duration,
}
/// 动态校准资源选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicCalibrateResourceOption {
    pub option_type: DynamicCalibrateType,
    pub string_value: String,
    pub timestamp: String,
    pub unit: String,
}
impl DynamicCalibrateResourceOption {
    pub fn restore(&self) -> String {
        match self.option_type {
            DynamicCalibrateType::StartTime => format!("START_TIME {}", self.timestamp),
            DynamicCalibrateType::EndTime => format!("END_TIME {}", self.timestamp),
            DynamicCalibrateType::Duration if !self.string_value.is_empty() => {
                format!("DURATION {}", quote_string(&self.string_value))
            }
            DynamicCalibrateType::Duration => {
                format!(
                    "DURATION INTERVAL {} {}",
                    self.timestamp,
                    self.unit.to_uppercase()
                )
            }
        }
    }
}
/// 校准资源语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CalibrateResourceStmt {
    pub options: Vec<DynamicCalibrateResourceOption>,
    pub resource_type: CalibrateResourceType,
}
impl CalibrateResourceStmt {
    pub fn restore(&self) -> String {
        let mut sql = format!("CALIBRATE RESOURCE{}", self.resource_type.restore());
        for option in &self.options {
            sql.push(' ');
            sql.push_str(&option.restore());
        }
        sql
    }
}

/// DROP查询监视语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropQueryWatchStmt {
    pub int_value: i64,
    pub group_name: String,
    pub group_expression: Option<String>,
}
impl DropQueryWatchStmt {
    pub fn restore(&self) -> String {
        if !self.group_name.is_empty() {
            format!(
                "QUERY WATCH REMOVE RESOURCE GROUP {}",
                quote_name(&self.group_name)
            )
        } else if let Some(expr) = &self.group_expression {
            format!("QUERY WATCH REMOVE RESOURCE GROUP {expr}")
        } else {
            format!("QUERY WATCH REMOVE {}", self.int_value)
        }
    }
}
/// QUERY WATCH ADD 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddQueryWatchStmt {
    pub options: Vec<QueryWatchOption>,
}
impl AddQueryWatchStmt {
    pub fn restore(&self) -> String {
        format!(
            "QUERY WATCH ADD{}",
            self.options
                .iter()
                .map(|o| format!(" {}", o.restore()))
                .collect::<String>()
        )
    }
}
/// 查询监视资源组选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryWatchResourceGroupOption {
    pub group_name: String,
    pub group_expression: Option<String>,
}
impl QueryWatchResourceGroupOption {
    pub fn restore(&self) -> String {
        format!(
            "RESOURCE GROUP {}",
            self.group_expression
                .clone()
                .unwrap_or_else(|| quote_name(&self.group_name))
        )
    }
}
/// 查询监视文本类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueryWatchTextType {
    #[default]
    Similar,
    Plan,
    Exact,
}
/// 查询监视文本选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryWatchTextOption {
    pub watch_type: QueryWatchTextType,
    pub pattern: String,
    pub type_specified: bool,
}
impl QueryWatchTextOption {
    pub fn restore(&self) -> String {
        if self.type_specified {
            format!(
                "SQL TEXT {} TO {}",
                match self.watch_type {
                    QueryWatchTextType::Similar => "SIMILAR",
                    QueryWatchTextType::Plan => "PLAN",
                    QueryWatchTextType::Exact => "EXACT",
                },
                self.pattern
            )
        } else {
            format!(
                "{} {}",
                if self.watch_type == QueryWatchTextType::Plan {
                    "PLAN DIGEST"
                } else {
                    "SQL DIGEST"
                },
                self.pattern
            )
        }
    }
}

/// Visitor contract shared by all executable nodes in this module.
/// `enter == true` skips children, matching Go's Visitor convention.
/// 杂项语句访问者：按节点名 enter/leave。
pub trait MiscVisitor {
    fn enter(&mut self, node_type: &'static str) -> bool;
    fn leave(&mut self, node_type: &'static str) -> bool;
}

/// 可被 MiscVisitor 遍历的杂项语句。
pub trait MiscVisitable {
    fn accept<V: MiscVisitor>(&mut self, visitor: &mut V) -> bool;
}

/// 敏感语句特质：提供脱敏后的 secure_sql/secure_text。
pub trait SensitiveStatement {
    fn secure_sql(&self) -> String;
}
macro_rules! sensitive_statement {
    ($($type:ty),+ $(,)?) => {$(
        impl SensitiveStatement for $type {
            fn secure_sql(&self) -> String { <$type>::secure_text(self) }
        }
    )+};
}
sensitive_statement!(
    SetPwdStmt,
    SetStmt,
    CreateUserStmt,
    AlterUserStmt,
    GrantStmt,
    GrantRoleStmt,
    TrafficStmt,
    BRIEStmt
);

macro_rules! leaf_visitable {
    ($($type:ty),+ $(,)?) => {$(
        impl MiscVisitable for $type {
            fn accept<V: MiscVisitor>(&mut self, visitor: &mut V) -> bool {
                let name = std::any::type_name::<Self>();
                let _skip_children = visitor.enter(name);
                visitor.leave(name)
            }
        }
    )+};
}

leaf_visitable!(
    AuthOption,
    PlanReplayerStmt,
    QueryWatchOption,
    SetPwdStmt,
    PrepareStmt,
    DeallocateStmt,
    ExecuteStmt,
    BeginStmt,
    BinlogStmt,
    CommitStmt,
    RollbackStmt,
    UseStmt,
    VariableAssignment,
    SetStmt,
    SetConfigStmt,
    SetSessionStatesStmt,
    SavepointStmt,
    ReleaseSavepointStmt,
    SetRoleStmt,
    SetDefaultRoleStmt,
    CreateUserStmt,
    AlterUserStmt,
    AlterInstanceStmt,
    AlterRangeStmt,
    DropUserStmt,
    StringOrUserVar,
    RecommendIndexStmt,
    CreateBindingStmt,
    DropBindingStmt,
    SetBindingStmt,
    CreateStatisticsStmt,
    DropStatisticsStmt,
    DoStmt,
    AdminStmt,
    TrafficStmt,
    CompactTableStmt,
    FlushStmt,
    KillStmt,
    TraceStmt,
    ExplainForStmt,
    ExplainStmt,
    SetCharsetStmt,
    ShutdownStmt,
    RestartStmt,
    HelpStmt,
    RenameUserStmt,
    PrivElem,
    RevokeStmt,
    RevokeRoleStmt,
    GrantStmt,
    GrantProxyStmt,
    GrantRoleStmt,
    BRIEStmt,
    ImportIntoActionStmt,
    CancelDistributionJobStmt,
    TableOptimizerHint,
    SetResourceGroupStmt,
    CalibrateResourceStmt,
    DynamicCalibrateResourceOption,
    DropQueryWatchStmt,
    AddQueryWatchStmt,
    QueryWatchTextOption,
);
