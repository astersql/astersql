// Copyright 2024 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// dump schema（库/表/视图）导入执行器。
//
// 按计划依次创建数据库、表（自动补 IF NOT EXISTS）与视图；
// 视图顺序由 view_import 依赖图决定。提供查询已有库表对象的辅助方法。
use crate::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// schema 语句类别：建库 / 建表 / 建视图。
pub enum SchemaStmtType {
    SchemaCreateDatabase,
    SchemaCreateTable,
    SchemaCreateView,
}
impl std::fmt::Display for SchemaStmtType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SchemaCreateDatabase => "import database schema",
            Self::SchemaCreateTable => "import table schema",
            Self::SchemaCreateView => "import view schema",
        })
    }
}
#[derive(Clone, Debug)]
/// 单次 schema 导入任务描述。
pub struct SchemaJob {
    /// 目标库名。
    pub db_name: String,
    /// 目标表/视图名（建库时为空）。
    pub tbl_name: String,
    /// 语句类型。
    pub stmt_type: SchemaStmtType,
    /// 原始 SQL 文本。
    pub sql_str: String,
}
/// 执行 DDL/查询的数据库抽象（测试可注入 RecordingDatabase）。
pub trait SchemaDatabase: Send + Sync {
    /// 执行一条 SQL。
    fn execute(&self, sql: &str) -> Result<(), MydumpError>;
    /// 执行查询并返回字符串行列。
    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, MydumpError>;
}
/// schema 导入器：持有 DB、对象存储与并发度配置。
pub struct SchemaImporter {
    db: Arc<dyn SchemaDatabase>,
    store: Arc<dyn Storage>,
    concurrency: usize,
}
/// 构造 SchemaImporter；concurrency 至少为 1。
pub fn NewSchemaImporter(
    db: Arc<dyn SchemaDatabase>,
    store: Arc<dyn Storage>,
    concurrency: usize,
) -> SchemaImporter {
    SchemaImporter {
        db,
        store,
        concurrency: concurrency.max(1),
    }
}
impl SchemaImporter {
    /// 执行完整导入：建库 → 建表 → 按依赖建视图。
    pub fn Run(&self, dbs: &[MDDatabaseMeta]) -> Result<(), MydumpError> {
        let plan = NewSchemaImportPlan(self.store.as_ref(), dbs)?;
        self.importDatabases(&plan.db_metas)?;
        self.importTables(&plan.db_metas)?;
        self.importViews(&plan)
    }
    /// 为每个库提交 CREATE DATABASE 任务。
    pub fn importDatabases(&self, dbs: &[MDDatabaseMeta]) -> Result<(), MydumpError> {
        let existing = self.getExistingDatabases()?;
        for db in dbs {
            if existing.contains(&db.name.to_ascii_lowercase()) {
                continue;
            }
            let sql = db.GetSchema(self.store.as_ref());
            self.runJob(
                &SchemaJob {
                    db_name: db.name.clone(),
                    tbl_name: String::new(),
                    stmt_type: SchemaStmtType::SchemaCreateDatabase,
                    sql_str: sql.clone(),
                },
                &createIfNotExistsStmt(&sql, &db.name, "")?,
            )?
        }
        Ok(())
    }
    /// 为每张表读取 schema 并执行（必要时拆成 IF NOT EXISTS 语句）。
    pub fn importTables(&self, dbs: &[MDDatabaseMeta]) -> Result<(), MydumpError> {
        for db in dbs {
            for table in &db.tables {
                if table.schema_file.file_meta.path.is_empty() {
                    // 与 Go SchemaImporter.importTables 对齐：无 schema 文件时
                    // 查询下游对象；SHOW CREATE 成功即视为已存在并跳过建表。
                    self.db
                        .query(&format!(
                            "SHOW CREATE TABLE `{}`.`{}`",
                            db.name.replace('`', "``"),
                            table.name.replace('`', "``")
                        ))
                        .map_err(|error| {
                            MydumpError::Schema(format!(
                                "checking existing table {}.{}: {error}",
                                db.name, table.name
                            ))
                        })?;
                    continue;
                }
                let sql = table.GetSchema(self.store.as_ref())?;
                let stmts = createIfNotExistsStmt(&sql, &db.name, &table.name)?;
                self.runJob(
                    &SchemaJob {
                        db_name: db.name.clone(),
                        tbl_name: table.name.clone(),
                        stmt_type: SchemaStmtType::SchemaCreateTable,
                        sql_str: sql,
                    },
                    &stmts,
                )?
            }
        }
        Ok(())
    }
    /// 按 view_plan.ordered 顺序创建视图。
    pub fn importViews(&self, plan: &SchemaImportPlan) -> Result<(), MydumpError> {
        if let Some(views) = &plan.view_plan {
            let (existing_non_views, existing_views) = self.loadExistingViewDependencies(views)?;
            validateViewImportPlan(
                views,
                &unionTableNames(&[existing_non_views.clone(), existing_views.clone()]),
            )?;
            for key in &views.ordered {
                let node = &views.nodes[key];
                let normalized = normalizeTableName(&key.schema, &key.name);
                if existing_views.contains(&normalized) {
                    continue;
                }
                if existing_non_views.contains(&normalized) {
                    return Err(MydumpError::Schema(format!(
                        "downstream non-view object already exists for view '{}.{}'",
                        key.schema, key.name
                    )));
                }
                self.runJob(
                    &SchemaJob {
                        db_name: key.schema.clone(),
                        tbl_name: key.name.clone(),
                        stmt_type: SchemaStmtType::SchemaCreateView,
                        sql_str: node.create_sql.clone(),
                    },
                    &createIfNotExistsStmt(&node.create_sql, &key.schema, &key.name)?,
                )?
            }
        }
        Ok(())
    }
    /// 运行建表任务（自动补 IF NOT EXISTS）。
    pub fn runCreateTableJob(&self, job: &SchemaJob) -> Result<(), MydumpError> {
        self.runJob(
            job,
            &createIfNotExistsStmt(&job.sql_str, &job.db_name, &job.tbl_name)?,
        )
    }
    /// 运行通用任务（直接执行 sql_str）。
    pub fn runCommonJob(&self, job: &SchemaJob) -> Result<(), MydumpError> {
        self.runJob(
            job,
            &createIfNotExistsStmt(&job.sql_str, &job.db_name, &job.tbl_name)?,
        )
    }
    /// 逐条执行解析后的语句；空列表保持 no-op，与 Go runJob 一致。
    pub fn runJob(&self, job: &SchemaJob, stmts: &[String]) -> Result<(), MydumpError> {
        for stmt in stmts {
            self.db.execute(stmt).map_err(|error| {
                MydumpError::Schema(format!(
                    "{} {}.{}: {error}",
                    job.stmt_type, job.db_name, job.tbl_name
                ))
            })?
        }
        Ok(())
    }
    /// SHOW DATABASES，返回小写库名集合。
    pub fn getExistingDatabases(&self) -> Result<HashSet<String>, MydumpError> {
        self.getExistingSchemas("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA")
    }
    /// 判断库中是否存在同名表/视图。
    pub fn isTableExist(&self, db: &str, table: &str) -> Result<bool, MydumpError> {
        Ok(!self
            .db
            .query(&format!(
                "SHOW TABLES FROM `{}` LIKE '{}'",
                db.replace('`', "``"),
                table.replace('\'', "''")
            ))?
            .is_empty())
    }
    /// SHOW FULL TABLES：名字 → 是否为 VIEW。
    pub fn getExistingObjectTypes(&self, db: &str) -> Result<HashMap<String, bool>, MydumpError> {
        let mut result = HashMap::new();
        for row in self.db.query(&format!(
            "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = '{}'",
            db.replace('\'', "''")
        ))? {
            if row.len() >= 2 {
                result.insert(
                    row[0].to_ascii_lowercase(),
                    row[1].eq_ignore_ascii_case("VIEW"),
                );
            }
        }
        Ok(result)
    }
    /// 执行自定义查询，展平为字符串集合。
    pub fn getExistingSchemas(&self, query: &str) -> Result<HashSet<String>, MydumpError> {
        Ok(self
            .queryStringRows(query)?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|name| name.to_ascii_lowercase())
            .collect())
    }
    /// 转发到 SchemaDatabase::query。
    pub fn queryStringRows(&self, query: &str) -> Result<Vec<Vec<String>>, MydumpError> {
        self.db.query(query)
    }

    fn loadExistingViewDependencies(
        &self,
        plan: &ViewImportPlan,
    ) -> Result<(TableNameSet, TableNameSet), MydumpError> {
        let mut schemas = HashSet::new();
        for node in plan.nodes.values() {
            schemas.insert(node.key.schema.to_ascii_lowercase());
            schemas.extend(
                node.deps
                    .iter()
                    .map(|dependency| dependency.schema.to_ascii_lowercase()),
            );
            schemas.extend(
                node.external_deps
                    .iter()
                    .map(|dependency| dependency.schema.to_ascii_lowercase()),
            );
        }
        let (mut non_views, mut views) = (TableNameSet::new(), TableNameSet::new());
        for schema in schemas {
            for (name, is_view) in self.getExistingObjectTypes(&schema)? {
                let key = normalizeTableName(&schema, &name);
                if is_view {
                    views.insert(key);
                } else {
                    non_views.insert(key);
                }
            }
        }
        Ok((non_views, views))
    }
}
/// 由库表名构造 TableName（不归一化大小写）。
pub fn tableKey(db: &str, table: &str) -> TableName {
    TableName {
        schema: db.into(),
        name: table.into(),
    }
}
/// 收集 dump 中全部表名（归一化）。
pub fn collectDumpTables(dbs: &[MDDatabaseMeta]) -> TableNameSet {
    dbs.iter()
        .flat_map(|db| {
            db.tables
                .iter()
                .map(move |t| normalizeTableName(&db.name, &t.name))
        })
        .collect()
}
/// 合并多个表名集合。
pub fn unionTableNames(sets: &[TableNameSet]) -> TableNameSet {
    sets.iter().flat_map(|s| s.iter().cloned()).collect()
}
/// 将 CREATE TABLE 改写为 IF NOT EXISTS（非 ANSI_QUOTES 模式）。
pub fn createIfNotExistsStmt(sql: &str, db: &str, table: &str) -> Result<Vec<String>, MydumpError> {
    createIfNotExistsStmtWithMode(sql, db, table, false)
}
/// 按模式拆分 SQL，并为首条 CREATE TABLE 补 IF NOT EXISTS。
pub fn createIfNotExistsStmtWithMode(
    sql: &str,
    db: &str,
    table: &str,
    ignore_destructive_ddl: bool,
) -> Result<Vec<String>, MydumpError> {
    let statements = split_sql(sql)?;
    let drop = Regex::new(r"(?i)^\s*DROP\s+(?:TABLE|DATABASE)\b").unwrap();
    let create_database = Regex::new(
        r"(?is)\bCREATE\s+DATABASE(?:\s+IF\s+NOT\s+EXISTS)?\s+(?:`(?:``|[^`])+`|[^\s;]+)",
    )
    .unwrap();
    let create_table = Regex::new(r"(?is)\bCREATE\s+TABLE(?:\s+IF\s+NOT\s+EXISTS)?\s+(?:(?:`(?:``|[^`])+`|[^.`\s(]+)\s*\.\s*)?(?:`(?:``|[^`])+`|[^\s(]+)").unwrap();
    let create_view = Regex::new(
        r"(?is)\bVIEW\s+(?:`(?:``|[^`])+`|[^.`\s(]+)(?:\s*\.\s*(?:`(?:``|[^`])+`|[^\s(]+))?",
    )
    .unwrap();
    let valid_statement = Regex::new(
        r"(?is)\b(?:CREATE|DROP|SET|ALTER|USE|GRANT|REVOKE|RENAME|TRUNCATE)\b|^\s*/\*!.*\*/\s*;?\s*$",
    )
    .unwrap();
    let mut rewritten = Vec::with_capacity(statements.len());
    for mut statement in statements {
        if !valid_statement.is_match(&statement) {
            return Err(MydumpError::Syntax(format!(
                "invalid schema statement: {statement}"
            )));
        }
        if ignore_destructive_ddl && drop.is_match(&statement) {
            continue;
        }
        if create_database.is_match(&statement) {
            statement = create_database
                .replace(
                    &statement,
                    format!("CREATE DATABASE IF NOT EXISTS `{}`", quote_ident(db)),
                )
                .into_owned();
        } else if create_table.is_match(&statement) {
            statement = create_table
                .replace(
                    &statement,
                    format!(
                        "CREATE TABLE IF NOT EXISTS `{}`.`{}`",
                        quote_ident(db),
                        quote_ident(table)
                    ),
                )
                .into_owned();
            statement = normalizeCreateTableColumns(&statement)?;
        } else if statement.to_ascii_lowercase().contains("create")
            && create_view.is_match(&statement)
        {
            statement = create_view
                .replace(
                    &statement,
                    format!("VIEW `{}`.`{}`", quote_ident(db), quote_ident(table)),
                )
                .into_owned();
        }
        rewritten.push(statement);
    }
    Ok(rewritten)
}

/// 将 CREATE TABLE 顶层列定义恢复成 TiDB AST formatter 的标识符与逗号格式。
/// 约束定义保持原样；字符串、注释及类型参数中的逗号不会被拆分。
fn normalizeCreateTableColumns(statement: &str) -> Result<String, MydumpError> {
    let Some(open) = statement.find('(') else {
        return Ok(statement.to_owned());
    };
    let bytes = statement.as_bytes();
    let (mut depth, mut quote, mut close) = (0usize, None, None);
    for (offset, &byte) in bytes[open..].iter().enumerate() {
        if let Some(active) = quote {
            if byte == active && bytes.get(open + offset.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                close = Some(open + offset);
                break;
            }
        }
    }
    let close =
        close.ok_or_else(|| MydumpError::Syntax("unterminated CREATE TABLE body".into()))?;
    let body = &statement[open + 1..close];
    let mut definitions = Vec::new();
    let (mut start, mut nested, mut active_quote) = (0usize, 0usize, None);
    for (index, &byte) in body.as_bytes().iter().enumerate() {
        if let Some(active) = active_quote {
            if byte == active && body.as_bytes().get(index.wrapping_sub(1)) != Some(&b'\\') {
                active_quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            active_quote = Some(byte);
        } else if byte == b'(' {
            nested += 1;
        } else if byte == b')' {
            nested = nested.saturating_sub(1);
        } else if byte == b',' && nested == 0 {
            definitions.push(&body[start..index]);
            start = index + 1;
        }
    }
    definitions.push(&body[start..]);

    let constraints = [
        "PRIMARY",
        "UNIQUE",
        "KEY",
        "INDEX",
        "CONSTRAINT",
        "FOREIGN",
        "CHECK",
        "FULLTEXT",
        "SPATIAL",
    ];
    let identifier = Regex::new(r"^([A-Za-z_][A-Za-z0-9_$]*)(\s+.*)$").unwrap();
    let normalized = definitions
        .into_iter()
        .map(|definition| {
            let definition = definition.trim();
            let first = definition
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches('`')
                .to_ascii_uppercase();
            if definition.starts_with('`') || constraints.contains(&first.as_str()) {
                return definition.to_owned();
            }
            identifier.replace(definition, "`$1`$2").into_owned()
        })
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "{}({}){}",
        &statement[..open],
        normalized,
        &statement[close + 1..]
    ))
}

fn quote_ident(identifier: &str) -> String {
    identifier.replace('`', "``")
}
/// 按分号拆分 SQL，尊重引号内分号；未闭合引号报错。
fn split_sql(sql: &str) -> Result<Vec<String>, MydumpError> {
    let mut out = Vec::new();
    let (mut start, mut quote) = (0, None);
    let bytes = sql.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if b == q && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
                quote = None
            }
        } else if matches!(b, b'\'' | b'"' | b'`') {
            quote = Some(b)
        } else if b == b';' {
            let stmt = sql[start..=i].trim();
            if !stmt.is_empty() {
                out.push(stmt.into())
            }
            start = i + 1
        }
    }
    if quote.is_some() {
        return Err(MydumpError::Syntax(
            "unterminated quote in schema SQL".into(),
        ));
    }
    let rest = sql[start..].trim();
    if !rest.is_empty() {
        out.push(rest.into())
    }
    Ok(out)
}
/// 自由函数包装。
pub fn getExistingDatabases(i: &SchemaImporter) -> Result<HashSet<String>, MydumpError> {
    i.getExistingDatabases()
}
/// 自由函数包装。
pub fn getExistingObjectTypes(
    i: &SchemaImporter,
    db: &str,
) -> Result<HashMap<String, bool>, MydumpError> {
    i.getExistingObjectTypes(db)
}
/// 自由函数包装。
pub fn getExistingSchemas(i: &SchemaImporter, q: &str) -> Result<HashSet<String>, MydumpError> {
    i.getExistingSchemas(q)
}
/// 自由函数包装。
pub fn isTableExist(i: &SchemaImporter, d: &str, t: &str) -> Result<bool, MydumpError> {
    i.isTableExist(d, t)
}
/// 自由函数包装。
pub fn queryStringRows(i: &SchemaImporter, q: &str) -> Result<Vec<Vec<String>>, MydumpError> {
    i.queryStringRows(q)
}
/// 自由函数包装。
pub fn runCommonJob(i: &SchemaImporter, j: &SchemaJob) -> Result<(), MydumpError> {
    i.runCommonJob(j)
}
/// 自由函数包装。
pub fn runCreateTableJob(i: &SchemaImporter, j: &SchemaJob) -> Result<(), MydumpError> {
    i.runCreateTableJob(j)
}
/// 自由函数包装。
pub fn runJob(i: &SchemaImporter, j: &SchemaJob, s: &[String]) -> Result<(), MydumpError> {
    i.runJob(j, s)
}
/// SchemaStmtType 的 Display 字符串。
fn String(kind: SchemaStmtType) -> String {
    kind.to_string()
}
/// 转发到 view_import::normalizeTableName。
fn normalizeTableName(db: &str, table: &str) -> TableName {
    crate::view_import::normalizeTableName(db, table)
}
