// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// MySQL 复制过滤规则兼容层：将旧版 Do/Ignore DB/Table 规则转为统一 Filter。
//
// MySQL replication 过滤（如 `--replicate-do-db`）与 TiDB 的表过滤器语法不同；
// 本模块解析并组合 schema/table 两侧规则，供迁移与兼容路径使用。

use super::{
    All, Filter, FilterError, newRegexpMatcher, stringMatcher, tableFilter, tableRule, trueMatcher,
};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};

/// Match Go's `strings.ToLower`, which applies a one-rune simple case mapping.
fn goToLower(value: &str) -> String {
    value
        .chars()
        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 库表标识：Schema 为库名，Name 为表名；Name 为空时仅表示 schema。
pub struct Table {
    pub Schema: String,
    pub Name: String,
}

impl Table {
    /// 构造库表标识。
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            Schema: schema.into(),
            Name: name.into(),
        }
    }

    /// 按 Schema 再 Name 的字典序比较，用于有序列表。
    pub fn lessThan(&self, other: &Table) -> bool {
        self.Schema < other.Schema || (self.Schema == other.Schema && self.Name < other.Name)
    }

    /// 堆分配克隆，对齐 Go 的 `Clone()` 返回指针语义。
    pub fn Clone(&self) -> Box<Table> {
        Box::new(Clone::clone(self))
    }
}

impl Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.Name.is_empty() {
            write!(f, "`{}`", self.Schema)
        } else {
            write!(f, "`{}`.`{}`", self.Schema, self.Name)
        }
    }
}

#[derive(Debug, Default)]
/// 旧版 MySQL 复制过滤四元组：Do/Ignore 的库与表列表。
///
/// Do* 非空时优先作白名单；否则 Ignore* 作黑名单（再默认放行）。
pub struct MySQLReplicationRules {
    pub DoTables: Vec<Box<Table>>,
    pub DoDBs: Vec<String>,
    pub IgnoreTables: Vec<Box<Table>>,
    pub IgnoreDBs: Vec<String>,
}

impl MySQLReplicationRules {
    /// 就地将规则中的库表名转为小写，便于大小写不敏感匹配。
    pub fn ToLower(rules: Option<&mut MySQLReplicationRules>) {
        let Some(rules) = rules else { return };
        for table in rules.DoTables.iter_mut().chain(&mut rules.IgnoreTables) {
            table.Schema = goToLower(&table.Schema);
            table.Name = goToLower(&table.Name);
        }
        for schema in rules.IgnoreDBs.iter_mut().chain(&mut rules.DoDBs) {
            *schema = goToLower(schema);
        }
    }
}

#[derive(Debug)]
/// 仅按 schema 名称集合过滤的 Filter。
struct schemasFilter {
    schemas: HashSet<String>,
}

impl Filter for schemasFilter {
    fn MatchTable(&self, schema: &str, _table: &str) -> bool {
        self.MatchSchema(schema)
    }
    fn MatchSchema(&self, schema: &str) -> bool {
        self.schemas.contains(schema)
    }
    fn toLower(&self) -> Box<dyn Filter> {
        Box::new(schemasFilter {
            schemas: self.schemas.iter().map(|value| goToLower(value)).collect(),
        })
    }
}

/// 由 schema 名列表构造精确匹配过滤器。
pub fn NewSchemasFilter(schemas: Vec<String>) -> Box<dyn Filter> {
    Box::new(schemasFilter {
        schemas: schemas.into_iter().collect(),
    })
}

#[derive(Debug)]
/// 按 schema→表名集合映射过滤的 Filter。
struct tablesFilter {
    schemas: HashMap<String, HashSet<String>>,
}

impl Filter for tablesFilter {
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        self.schemas
            .get(schema)
            .is_some_and(|tables| tables.contains(table))
    }
    fn MatchSchema(&self, schema: &str) -> bool {
        self.schemas.contains_key(schema)
    }
    fn toLower(&self) -> Box<dyn Filter> {
        let mut schemas: HashMap<String, HashSet<String>> = HashMap::new();
        for (schema, tables) in &self.schemas {
            schemas
                .entry(goToLower(schema))
                .or_default()
                .extend(tables.iter().map(|table| goToLower(table)));
        }
        Box::new(tablesFilter { schemas })
    }
}

/// 由显式库表列表构造精确匹配过滤器。
pub fn NewTablesFilter(tables: Vec<Table>) -> Box<dyn Filter> {
    let mut schemas: HashMap<String, HashSet<String>> = HashMap::new();
    for table in tables {
        schemas.entry(table.Schema).or_default().insert(table.Name);
    }
    Box::new(tablesFilter { schemas })
}

#[derive(Debug)]
/// 两个 Filter 的合取：库表与 schema 匹配均需两侧同时成立。
struct bothFilter {
    a: Box<dyn Filter>,
    b: Box<dyn Filter>,
}

impl Filter for bothFilter {
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        self.a.MatchTable(schema, table) && self.b.MatchTable(schema, table)
    }
    fn MatchSchema(&self, schema: &str) -> bool {
        self.a.MatchSchema(schema) && self.b.MatchSchema(schema)
    }
    fn toLower(&self) -> Box<dyn Filter> {
        Box::new(bothFilter {
            a: self.a.toLower(),
            b: self.b.toLower(),
        })
    }
}

// 将旧版通配/`~` 正则模式转为统一 matcher。
/// 解析 MySQL 复制风格模式：空串报错；`~` 前缀为正则；否则 glob→正则或精确串。
fn matcherFromLegacyPattern(pattern: &str) -> Result<Box<dyn super::matcher>, FilterError> {
    if pattern.is_empty() {
        return Err(FilterError("pattern cannot be empty".into()));
    }
    if let Some(pattern) = pattern.strip_prefix('~') {
        return newRegexpMatcher(pattern);
    }
    if !pattern.contains(['?', '*', '[']) {
        return Ok(Box::new(stringMatcher(pattern.into())));
    }
    let mut pattern = regex::escape(pattern);
    for (from, to) in [
        (r"\*", ".*"),
        (r"\?", "."),
        (r"\[!", "[^"),
        (r"\[", "["),
        (r"\]", "]"),
    ] {
        pattern = pattern.replace(from, to);
    }
    newRegexpMatcher(&format!("(?s)^{pattern}$"))
}

/// 把可选的 MySQLReplicationRules 转为统一 Filter；`None` 等价于匹配全部。
pub fn ParseMySQLReplicationRules(
    rules: Option<&MySQLReplicationRules>,
) -> Result<Box<dyn Filter>, FilterError> {
    let Some(rules) = rules else { return Ok(All()) };

    // DoDBs 优先白名单；否则用 IgnoreDBs 黑名单（positive=false）并稍后补默认放行规则。
    let (schemas, positive) = if rules.DoDBs.is_empty() {
        (&rules.IgnoreDBs, false)
    } else {
        (&rules.DoDBs, true)
    };
    let mut schema_rules = Vec::with_capacity(schemas.len() + usize::from(!positive));
    for schema in schemas {
        schema_rules.push(tableRule {
            schema: matcherFromLegacyPattern(schema)?,
            table: Box::new(trueMatcher),
            positive,
        });
    }
    if !positive {
        schema_rules.push(tableRule {
            schema: Box::new(trueMatcher),
            table: Box::new(trueMatcher),
            positive: true,
        });
    }

    // 表侧同理：DoTables 白名单，否则 IgnoreTables 黑名单 + 默认放行。
    let (tables, positive) = if rules.DoTables.is_empty() {
        (&rules.IgnoreTables, false)
    } else {
        (&rules.DoTables, true)
    };
    let mut table_rules = Vec::with_capacity(tables.len() + usize::from(!positive));
    for table in tables {
        table_rules.push(tableRule {
            schema: matcherFromLegacyPattern(&table.Schema)?,
            table: matcherFromLegacyPattern(&table.Name)?,
            positive,
        });
    }
    if !positive {
        table_rules.push(tableRule {
            schema: Box::new(trueMatcher),
            table: Box::new(trueMatcher),
            positive: true,
        });
    }

    Ok(Box::new(bothFilter {
        a: Box::new(tableFilter(schema_rules)),
        b: Box::new(tableFilter(table_rules)),
    }))
}
