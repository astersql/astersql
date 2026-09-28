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

// DDL 轻量解析：从 CREATE TABLE / CREATE INDEX 提取表结构与唯一性信息。
//
// 列 COMMENT 中可用 `[[range=...;step=...;set=...]]` 规则约束生成数据。
// 不做完整 SQL 语法分析，仅按括号/引号深度切分顶层逗号。

use crate::config::ImporterError;
use crate::data::Datum;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// 唯一值默认步长。
pub const DEFAULT_STEP: i64 = 1;

/// 导入工具支持的列类型族。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    TinyInt,
    SmallInt,
    Int,
    BigInt,
    Varchar,
    String,
    Blob,
    Float,
    Double,
    Decimal,
    Date,
    DateTime,
    Timestamp,
    Time,
    Year,
}

/// 列类型描述：种类、是否 UNSIGNED、长度（如 VARCHAR(n)）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldType {
    pub kind: FieldKind,
    pub unsigned: bool,
    pub length: usize,
}

/// 单列元数据：类型、生成规则、绑定的 `Datum`。
pub struct Column {
    pub data: Arc<Datum>,
    pub field_type: FieldType,
    pub name: String,
    pub comment: String,
    pub minimum: String,
    pub maximum: String,
    pub set: Vec<String>,
    pub index: usize,
    pub step: i64,
}

impl std::fmt::Debug for Column {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Column")
            .field("index", &self.index)
            .field("name", &self.name)
            .field("field_type", &self.field_type)
            .field("minimum", &self.minimum)
            .field("maximum", &self.maximum)
            .field("step", &self.step)
            .field("set", &self.set)
            .finish()
    }
}

impl Column {
    /// 解析单条规则：`range=min,max` / `step=n` / `set=a,b,c`。
    fn parse_rule(&mut self, rule: &str) -> Result<(), ImporterError> {
        let mut fields = rule.split('=');
        let (Some(key), Some(value), None) = (fields.next(), fields.next(), fields.next()) else {
            return Ok(());
        };
        let value = value.trim();
        match key.trim() {
            "range" => {
                let fields = value.split(',').map(str::trim).collect::<Vec<_>>();
                match fields.as_slice() {
                    [minimum] => self.minimum = (*minimum).to_owned(),
                    [minimum, maximum] => {
                        self.minimum = (*minimum).to_owned();
                        self.maximum = (*maximum).to_owned();
                    }
                    _ => {}
                }
            }
            "step" => {
                self.step = value
                    .parse()
                    .map_err(|_| ImporterError::Parse(format!("invalid step {value}")))?;
            }
            "set" => self
                .set
                .extend(value.split(',').map(|value| value.trim().to_owned())),
            _ => {}
        }
        Ok(())
    }

    /// 从 COMMENT 中提取 `[[...]]` 块并逐条解析规则。
    fn parse_comment_rules(&mut self) -> Result<(), ImporterError> {
        let Some(start) = self.comment.find("[[") else {
            return Ok(());
        };
        let Some(end) = self.comment.find("]]") else {
            return Ok(());
        };
        if start >= end {
            return Ok(());
        }
        let content = self.comment[start + 2..end].to_owned();
        for rule in content.split(';') {
            self.parse_rule(rule.trim())?;
        }
        Ok(())
    }
}

/// 表结构：列列表、普通/唯一索引集合、无符号列集合。
pub struct Table {
    pub indices: HashMap<String, Option<Arc<Column>>>,
    pub unique_indices: HashSet<String>,
    pub unsigned_columns: HashSet<String>,
    pub name: String,
    /// 用于 INSERT 列清单，如 `` `a`,`b` ``。
    pub column_list: String,
    pub columns: Vec<Arc<Column>>,
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

impl Table {
    /// 创建空表结构。
    pub fn new() -> Self {
        Self {
            indices: HashMap::new(),
            unique_indices: HashSet::new(),
            unsigned_columns: HashSet::new(),
            name: String::new(),
            column_list: String::new(),
            columns: Vec::new(),
        }
    }

    /// 按列名查找列。
    pub fn find_column(&self, name: &str) -> Option<Arc<Column>> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .cloned()
    }

    /// 生成带反引号转义的列名列表。
    fn build_column_list(&mut self) {
        self.column_list = self
            .columns
            .iter()
            .map(|column| format!("`{}`", column.name.replace('`', "``")))
            .collect::<Vec<_>>()
            .join(",");
    }
}

impl Display for Table {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "[table]name: {}", self.name)?;
        writeln!(f, "[table]columns:")?;
        for column in &self.columns {
            writeln!(f, "{column:?}")?;
        }
        writeln!(f, "[table]column list: {}", self.column_list)
    }
}

/// 规范化标识符：去反引号、取最后一段、转小写。
fn identifier(value: &str) -> String {
    value
        .trim()
        .trim_matches('`')
        .rsplit('.')
        .next()
        .unwrap_or(value)
        .trim_matches('`')
        .replace("``", "`")
        .to_ascii_lowercase()
}

/// 按顶层分隔符切分，忽略括号与引号内的分隔符。
fn split_top_level(value: &str, separator: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    let mut quote = None;
    let bytes = value.as_bytes();
    for (index, character) in value.char_indices() {
        if let Some(expected) = quote {
            if character == expected && (index == 0 || bytes[index - 1] != b'\\') {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => depth -= 1,
            character if character == separator && depth == 0 => {
                fields.push(value[start..index].trim().to_owned());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    fields.push(value[start..].trim().to_owned());
    fields
}

/// 从类型片段解析 `FieldType`（含 UNSIGNED 与长度）。
fn field_type(definition: &str) -> Result<FieldType, ImporterError> {
    let lower = definition.to_ascii_lowercase();
    let token = lower.split_whitespace().next().unwrap_or_default();
    let base = token.split('(').next().unwrap_or(token);
    let explicit_length = token
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(length, _)| length.split(',').next())
        .and_then(|length| length.parse().ok());
    let default_length = match base {
        "char" | "binary" => 1,
        "varchar" | "varbinary" => 5,
        "tinyblob" | "tinytext" => 255,
        "blob" | "text" => 65_535,
        "mediumblob" | "mediumtext" => 16_777_215,
        "longblob" | "longtext" => 4_294_967_295,
        _ => 16,
    };
    let length = explicit_length.unwrap_or(default_length);
    let kind = match base {
        "tinyint" => FieldKind::TinyInt,
        "smallint" => FieldKind::SmallInt,
        "int" | "integer" | "mediumint" => FieldKind::Int,
        "bigint" => FieldKind::BigInt,
        "varchar" | "char" => FieldKind::Varchar,
        "binary" | "varbinary" => FieldKind::String,
        "tinyblob" | "tinytext" | "blob" | "text" | "mediumblob" | "mediumtext" | "longblob"
        | "longtext" => FieldKind::Blob,
        "float" => FieldKind::Float,
        "double" | "real" => FieldKind::Double,
        "decimal" | "numeric" => FieldKind::Decimal,
        "date" => FieldKind::Date,
        "datetime" => FieldKind::DateTime,
        "timestamp" => FieldKind::Timestamp,
        "time" => FieldKind::Time,
        "year" => FieldKind::Year,
        _ => return Err(ImporterError::UnsupportedColumn(base.to_owned())),
    };
    Ok(FieldType {
        kind,
        unsigned: lower.split_whitespace().any(|token| token == "unsigned"),
        length,
    })
}

/// 从 PRIMARY KEY / UNIQUE / KEY / INDEX 定义中提取列名列表。
fn constraint_columns(definition: &str) -> Vec<String> {
    definition
        .find('(')
        .and_then(|start| definition.rfind(')').map(|end| (start, end)))
        .map(|(start, end)| {
            split_top_level(&definition[start + 1..end], ',')
                .into_iter()
                .map(|column| identifier(column.split('(').next().unwrap_or(&column)))
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 CREATE TABLE，填充列与唯一/无符号信息。
pub fn parse_table_sql(table: &mut Table, sql: &str) -> Result<(), ImporterError> {
    let lower = sql.trim().to_ascii_lowercase();
    let mut header_words = lower.split_whitespace();
    let valid_header = matches!(
        (header_words.next(), header_words.next()),
        (Some("create"), Some("table")) | (Some("create"), Some("temporary"))
    ) && (lower.starts_with("create table")
        || lower.starts_with("create temporary table"));
    if !valid_header {
        return Err(ImporterError::Parse("invalid statement".to_owned()));
    }
    let open = sql
        .find('(')
        .ok_or_else(|| ImporterError::Parse("missing '('".to_owned()))?;
    let close = sql
        .rfind(')')
        .ok_or_else(|| ImporterError::Parse("missing ')'".to_owned()))?;
    let header = sql[..open].split_whitespace().collect::<Vec<_>>();
    table.name = identifier(header.last().copied().unwrap_or_default());
    // Go's parseTable replaces the column slice on every parse while retaining
    // the maps allocated by newTable.
    table.columns.clear();
    for definition in split_top_level(&sql[open + 1..close], ',') {
        let lower = definition.to_ascii_lowercase();
        // 约束行：PRIMARY KEY / UNIQUE / KEY / INDEX，以及包含 UNIQUE 的
        // CONSTRAINT。外键约束对应 Go 解析器的未处理分支，不能登记为索引。
        let constraint = lower.starts_with("primary key")
            || lower.starts_with("unique")
            || lower.starts_with("key ")
            || lower.starts_with("index ")
            || lower.starts_with("constraint ")
            || lower.starts_with("foreign key")
            || lower.starts_with("check ");
        if constraint {
            if lower.starts_with("constraint ") && !lower.contains(" unique")
                || lower.starts_with("foreign key")
                || lower.starts_with("check ")
            {
                continue;
            }
            let unique =
                lower.contains("primary") || lower.contains("unique") || lower.starts_with("key ");
            for name in constraint_columns(&definition) {
                if unique {
                    table.unique_indices.insert(name);
                } else {
                    let column = table.find_column(&name);
                    table.indices.insert(name, column);
                }
            }
            continue;
        }
        let mut tokens = definition.split_whitespace();
        let name = identifier(tokens.next().unwrap_or_default());
        let type_definition = tokens.collect::<Vec<_>>().join(" ");
        let mut column = Column {
            data: Arc::new(Datum::new()),
            field_type: field_type(&type_definition)?,
            name: name.clone(),
            comment: extract_comment(&definition).unwrap_or_default(),
            minimum: String::new(),
            maximum: String::new(),
            set: Vec::new(),
            index: table.columns.len() + 1,
            step: DEFAULT_STEP,
        };
        if column.field_type.unsigned {
            table.unsigned_columns.insert(name.clone());
        }
        // 列内 PRIMARY KEY / AUTO_INCREMENT / UNIQUE 也记入唯一集合。
        if lower.contains("primary key")
            || lower.contains("auto_increment")
            || lower.contains(" unique")
        {
            table.unique_indices.insert(name.clone());
        }
        column.parse_comment_rules()?;
        table.columns.push(Arc::new(column));
    }
    table.build_column_list();
    Ok(())
}

/// 从列定义中提取 COMMENT '...' 字符串内容。
fn extract_comment(definition: &str) -> Option<String> {
    let lower = definition.to_ascii_lowercase();
    let position = lower.find("comment")? + "comment".len();
    let rest = definition[position..].trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let mut value = String::new();
    let mut characters = rest[1..].chars().peekable();
    while let Some(character) = characters.next() {
        if character == quote {
            if characters.peek() == Some(&quote) {
                characters.next();
                value.push(quote);
                continue;
            }
            return Some(value);
        }
        if character == '\\' {
            if let Some(escaped) = characters.next() {
                value.push(escaped);
                continue;
            }
        }
        value.push(character);
    }
    Some(value)
}

/// 解析 CREATE [UNIQUE] INDEX，校验表名后更新索引/唯一集合。
pub fn parse_index_sql(table: &mut Table, sql: &str) -> Result<(), ImporterError> {
    if sql.is_empty() {
        return Ok(());
    }
    let lower = sql.to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    if words.next() != Some("create") {
        return Err(ImporterError::Parse(
            "invalid create index statement".to_owned(),
        ));
    }
    let second = words.next();
    let (unique, index_keyword) = if second == Some("unique") {
        (true, words.next())
    } else {
        (false, second)
    };
    if index_keyword != Some("index") {
        return Err(ImporterError::Parse(
            "invalid create index statement".to_owned(),
        ));
    }
    let on = lower
        .find(" on ")
        .ok_or_else(|| ImporterError::Parse("missing ON".to_owned()))?;
    let rest = sql[on + 4..].trim_start();
    let open = rest
        .find('(')
        .ok_or_else(|| ImporterError::Parse("missing index columns".to_owned()))?;
    let table_name = identifier(&rest[..open]);
    if table_name != table.name {
        return Err(ImporterError::Parse(format!(
            "mismatch table name for create index - {} : {table_name}",
            table.name
        )));
    }
    for name in constraint_columns(rest) {
        if unique {
            table.unique_indices.insert(name);
        } else {
            let column = table.find_column(&name);
            table.indices.insert(name, column);
        }
    }
    Ok(())
}
