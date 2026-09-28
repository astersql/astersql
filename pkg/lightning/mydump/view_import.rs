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

// 视图 schema 依赖解析与导入顺序规划。
//
// 从 CREATE VIEW ... AS 查询中提取 FROM/JOIN 引用表，构建依赖图并做
// 拓扑排序；检测环依赖与缺失的外部对象。SchemaImportPlan 聚合库元数据
// 与可选的 ViewImportPlan。
use crate::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
/// 库表限定名（通常小写归一化后比较）。
pub struct TableName {
    /// 库名。
    pub schema: String,
    /// 表/视图名。
    pub name: String,
}
/// 表名集合。
pub type TableNameSet = HashSet<TableName>;
/// 将库表名转为小写 TableName。
pub fn normalizeTableName(schema: &str, name: &str) -> TableName {
    TableName {
        schema: schema.to_ascii_lowercase(),
        name: name.to_ascii_lowercase(),
    }
}
/// 向集合插入归一化后的表名。
pub fn add(set: &mut TableNameSet, table: TableName) {
    set.insert(normalizeTableName(&table.schema, &table.name));
}
/// 判断集合是否包含归一化后的表名。
pub fn has(set: &TableNameSet, table: &TableName) -> bool {
    set.contains(&normalizeTableName(&table.schema, &table.name))
}
#[derive(Clone, Debug)]
/// 解析后的单视图：自身 key、依赖列表与可执行 CREATE SQL。
pub struct ParsedViewSchema {
    /// 视图自身限定名。
    pub key: TableName,
    /// 查询中引用的依赖对象。
    pub deps: Vec<TableName>,
    /// 过滤 DROP 后、规范化 SET NAMES 的创建语句。
    pub create_sql: String,
}
#[derive(Clone, Debug)]
/// 依赖图中的视图节点。
pub struct ViewNode {
    /// 本视图限定名。
    pub key: TableName,
    /// 依赖的其他待导入视图。
    pub deps: Vec<TableName>,
    /// 依赖的 dump 外对象（需集群已存在）。
    pub external_deps: Vec<TableName>,
    /// 依赖本视图的后继视图。
    pub dependents: Vec<TableName>,
    /// 拓扑排序入度。
    pub indegree: usize,
    /// 创建 SQL。
    pub create_sql: String,
}
#[derive(Clone, Debug, Default)]
/// 视图导入计划：节点图与拓扑序。
pub struct ViewImportPlan {
    /// 视图节点映射。
    pub nodes: HashMap<TableName, ViewNode>,
    /// 可安全执行的创建顺序。
    pub ordered: Vec<TableName>,
}
/// TableName 字典序比较。
pub fn lessTableName(a: &TableName, b: &TableName) -> bool {
    a < b
}
/// 原地排序视图名列表。
pub fn sortViewNodes(nodes: &mut [TableName]) {
    nodes.sort()
}
#[derive(Clone, Debug, Default)]
/// 完整 schema 导入计划：库元数据 + 可选视图计划。
pub struct SchemaImportPlan {
    /// 待导入的库元数据列表。
    pub db_metas: Vec<MDDatabaseMeta>,
    /// 若有视图则含拓扑计划。
    pub view_plan: Option<ViewImportPlan>,
}
#[derive(Default)]
/// AST 遍历式依赖收集器（CTE 作用域感知）。
pub struct ViewDependencyCollector {
    current_schema: String,
    deps: TableNameSet,
    cte_name_scopes: Vec<HashSet<String>>,
}
impl ViewDependencyCollector {
    /// 进入新的 CTE 名称作用域。
    pub fn pushCTEScope(&mut self) {
        self.cte_name_scopes.push(HashSet::new())
    }
    /// 离开当前 CTE 作用域。
    pub fn popCTEScope(&mut self) {
        self.cte_name_scopes.pop();
    }
    /// 在当前作用域登记 CTE 名。
    pub fn recordCTEName(&mut self, name: &str) {
        if let Some(scope) = self.cte_name_scopes.last_mut() {
            scope.insert(name.to_ascii_lowercase());
        }
    }
    /// 自内向外查找是否为 CTE 名。
    pub fn isCTEName(&self, name: &str) -> bool {
        self.cte_name_scopes
            .iter()
            .rev()
            .any(|s| s.contains(&name.to_ascii_lowercase()))
    }
    /// 遇到表引用：非 CTE 则记入 deps。
    pub fn Enter(&mut self, name: &str) {
        if !self.isCTEName(name) {
            self.deps.insert(parse_name(name, &self.current_schema));
        }
    }
    /// 离开节点：登记 CTE 名并按需弹出 WITH 作用域。
    pub fn Leave(&mut self, cte_name: Option<&str>, has_with_clause: bool) {
        if let Some(name) = cte_name {
            self.recordCTEName(name);
        }
        if has_with_clause {
            self.popCTEScope();
        }
    }
}
/// 判断 SQL 是否以 WITH 开头。
pub fn hasWithClause(sql: &str) -> bool {
    sql.trim_start().to_ascii_lowercase().starts_with("with ")
}
/// 解析视图 schema SQL：提取 AS 后查询依赖并拼接可执行语句。
pub fn parseViewSchemaSQL(
    current_view: TableName,
    sql: &str,
) -> Result<ParsedViewSchema, MydumpError> {
    let create = Regex::new(r"(?is)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:ALGORITHM\s*=\s*\w+\s+)?(?:DEFINER\s*=\s*\S+\s+)?(?:SQL\s+SECURITY\s+\w+\s+)?VIEW\s+.+?\s+AS\s+(.+?)\s*;?\s*$").unwrap();
    let statements = split_view_statements(sql)?;
    let creates = statements
        .iter()
        .filter_map(|statement| {
            create
                .captures(statement)
                .map(|captures| (statement, captures))
        })
        .collect::<Vec<_>>();
    if creates.is_empty() {
        return Err(MydumpError::Schema(format!(
            "missing create view statement for `{}`.`{}`",
            current_view.schema, current_view.name
        )));
    }
    if creates.len() > 1 {
        return Err(MydumpError::Schema(format!(
            "multiple create view statements found for `{}`.`{}`",
            current_view.schema, current_view.name
        )));
    }
    // 收集 CTE 名，避免把 CTE 误当作外部表依赖。
    let query = creates[0].1.get(1).unwrap().as_str();
    // Go walks the parsed AST, so keywords inside literals and comments never
    // become table references. Mask those regions before applying the lexical
    // dependency matcher while retaining byte positions and quoted identifiers.
    let dependency_query = mask_view_query_literals_and_comments(query);
    let cte = Regex::new(r"(?i)(?:WITH\s+(?:RECURSIVE\s+)?|,)\s*`?([A-Za-z0-9_$]+)`?\s+AS\s*\(")
        .unwrap()
        .captures_iter(&dependency_query)
        .map(|c| c[1].to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let refs = Regex::new(
        r"(?i)\b(?:FROM|JOIN)\s+((?:`[^`]+`|[A-Za-z0-9_$]+)(?:\.(?:`[^`]+`|[A-Za-z0-9_$]+))?)",
    )
    .unwrap();
    let mut deps = TableNameSet::new();
    for capture in refs.captures_iter(&dependency_query) {
        let raw = capture[1].replace('`', "");
        let name = parse_name(&raw, &current_view.schema);
        if !cte.contains(&name.name) {
            deps.insert(name);
        }
    }
    Ok(ParsedViewSchema {
        key: normalizeTableName(&current_view.schema, &current_view.name),
        deps: deps.into_iter().collect(),
        create_sql: statements
            .into_iter()
            .filter(|statement| {
                let normalized = statement.trim_start().to_ascii_lowercase();
                !normalized.starts_with("drop table") && !normalized.starts_with("drop view")
            })
            .map(|statement| {
                Regex::new(r"(?i)^\s*SET\s+NAMES\s+([A-Za-z0-9_]+)\s*;?$")
                    .unwrap()
                    .replace(&statement, "SET NAMES '$1';")
                    .into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// Replace string literals and SQL comments with spaces before dependency
/// scanning. Newlines are preserved so line comments terminate exactly where
/// they do in MySQL. Backtick-quoted identifiers remain visible to the matcher.
fn mask_view_query_literals_and_comments(sql: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        Quote(u8),
        LineComment,
        BlockComment,
    }

    let bytes = sql.as_bytes();
    let mut masked = bytes.to_vec();
    let mut state = State::Code;
    let mut index = 0;
    while index < bytes.len() {
        match state {
            State::Code if matches!(bytes[index], b'\'' | b'"') => {
                masked[index] = b' ';
                state = State::Quote(bytes[index]);
                index += 1;
            }
            State::Code
                if bytes[index] == b'#'
                    || (bytes[index] == b'-'
                        && bytes.get(index + 1) == Some(&b'-')
                        && bytes
                            .get(index + 2)
                            .is_none_or(|byte| byte.is_ascii_whitespace())) =>
            {
                masked[index] = b' ';
                state = State::LineComment;
                index += 1;
            }
            State::Code if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') => {
                masked[index] = b' ';
                if index + 1 < masked.len() {
                    masked[index + 1] = b' ';
                }
                state = State::BlockComment;
                index += 2;
            }
            State::Code => index += 1,
            State::Quote(quote) => {
                masked[index] = if bytes[index] == b'\n' { b'\n' } else { b' ' };
                if bytes[index] == b'\\' {
                    if index + 1 < bytes.len() {
                        masked[index + 1] = if bytes[index + 1] == b'\n' {
                            b'\n'
                        } else {
                            b' '
                        };
                    }
                    index += 2;
                } else if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        masked[index + 1] = b' ';
                        index += 2;
                    } else {
                        state = State::Code;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::LineComment => {
                if bytes[index] == b'\n' {
                    state = State::Code;
                } else {
                    masked[index] = b' ';
                }
                index += 1;
            }
            State::BlockComment => {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    masked[index] = b' ';
                    masked[index + 1] = b' ';
                    state = State::Code;
                    index += 2;
                } else {
                    masked[index] = if bytes[index] == b'\n' { b'\n' } else { b' ' };
                    index += 1;
                }
            }
        }
    }
    // Only ASCII bytes are changed, so the original UTF-8 validity is kept.
    String::from_utf8(masked).expect("masking SQL preserves UTF-8")
}

/// 按分号拆分视图 SQL，尊重引号。
fn split_view_statements(sql: &str) -> Result<Vec<String>, MydumpError> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let bytes = sql.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if let Some(active) = quote {
            if byte == active && bytes.get(index.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b';' {
            let statement = sql[start..=index].trim();
            if !statement.is_empty() {
                statements.push(statement.to_owned());
            }
            start = index + 1;
        }
    }
    if quote.is_some() {
        return Err(MydumpError::Syntax("unterminated quote in view SQL".into()));
    }
    let rest = sql[start..].trim();
    if !rest.is_empty() {
        statements.push(rest.to_owned());
    }
    Ok(statements)
}
/// 解析 `db.tbl` 或默认当前库下的裸表名。
fn parse_name(raw: &str, current: &str) -> TableName {
    match raw.split_once('.') {
        Some((s, n)) => normalizeTableName(s, n),
        None => normalizeTableName(current, raw),
    }
}
/// 由解析结果构建依赖图并拓扑排序；环依赖时报错。
pub fn buildViewImportPlan(
    parsed: &[ParsedViewSchema],
    dump_tables: &TableNameSet,
) -> Result<ViewImportPlan, MydumpError> {
    let dump_tables = dump_tables
        .iter()
        .map(|table| normalizeTableName(&table.schema, &table.name))
        .collect::<HashSet<_>>();
    let views = parsed
        .iter()
        .map(|view| normalizeTableName(&view.key.schema, &view.key.name))
        .collect::<HashSet<_>>();
    if views.len() != parsed.len() {
        return Err(MydumpError::Schema("duplicate view definition".into()));
    }
    let mut nodes = HashMap::new();
    for view in parsed {
        let key = normalizeTableName(&view.key.schema, &view.key.name);
        // dump 内表不算依赖；其余视图依赖计入入度，外部对象单独列出。
        let (deps, external): (Vec<_>, Vec<_>) = view
            .deps
            .iter()
            .map(|dep| normalizeTableName(&dep.schema, &dep.name))
            .filter(|d| !dump_tables.contains(d))
            .partition(|d| views.contains(d));
        nodes.insert(
            key.clone(),
            ViewNode {
                key,
                indegree: deps.len(),
                deps,
                external_deps: external,
                dependents: Vec::new(),
                create_sql: view.create_sql.clone(),
            },
        );
    }
    let edges = nodes
        .values()
        .flat_map(|n| n.deps.iter().map(move |d| (d.clone(), n.key.clone())))
        .collect::<Vec<_>>();
    for (from, to) in edges {
        if let Some(node) = nodes.get_mut(&from) {
            node.dependents.push(to)
        }
    }
    // Kahn 拓扑：入度为 0 的视图入队，按名字排序保证稳定序。
    let mut ready = nodes
        .values()
        .filter(|n| n.indegree == 0)
        .map(|n| n.key.clone())
        .collect::<Vec<_>>();
    ready.sort();
    let mut ordered = Vec::new();
    while let Some(key) = ready.first().cloned() {
        ready.remove(0);
        ordered.push(key.clone());
        let dependents = nodes[&key].dependents.clone();
        for d in dependents {
            let n = nodes.get_mut(&d).unwrap();
            n.indegree -= 1;
            if n.indegree == 0 {
                ready.push(d);
                ready.sort()
            }
        }
    }
    if ordered.len() != nodes.len() {
        let mut cyclic = nodes
            .values()
            .filter(|node| node.indegree > 0)
            .map(|node| format!("`{}`.`{}`", node.key.schema, node.key.name))
            .collect::<Vec<_>>();
        cyclic.sort();
        return Err(MydumpError::Schema(format!(
            "cyclic view dependency: {}",
            cyclic.join(", ")
        )));
    }
    Ok(ViewImportPlan { nodes, ordered })
}
/// 校验外部依赖均存在于 existing 集合。
pub fn validateViewImportPlan(
    plan: &ViewImportPlan,
    existing: &TableNameSet,
) -> Result<(), MydumpError> {
    for node in plan.nodes.values() {
        for dep in &node.external_deps {
            if !existing.contains(dep) {
                return Err(MydumpError::Schema(format!(
                    "view {}.{} depends on missing object {}.{}",
                    node.key.schema, node.key.name, dep.schema, dep.name
                )));
            }
        }
    }
    Ok(())
}
/// 读取各库视图 schema，解析并生成 SchemaImportPlan。
pub fn NewSchemaImportPlan(
    store: &dyn Storage,
    dbs: &[MDDatabaseMeta],
) -> Result<SchemaImportPlan, MydumpError> {
    let mut parsed = Vec::new();
    let mut tables = TableNameSet::new();
    for db in dbs {
        for table in &db.tables {
            tables.insert(normalizeTableName(&db.name, &table.name));
        }
        for view in &db.views {
            let sql = view.GetSchema(store)?;
            parsed.push(parseViewSchemaSQL(
                normalizeTableName(&db.name, &view.name),
                &sql,
            )?);
        }
    }
    let view_plan = if parsed.is_empty() {
        None
    } else {
        Some(buildViewImportPlan(&parsed, &tables)?)
    };
    Ok(SchemaImportPlan {
        db_metas: dbs.to_vec(),
        view_plan,
    })
}
