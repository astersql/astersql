// Copyright 2026 AsterSQL.

// `pkg/executor/test/seqtest` 内存会话桩：支撑顺序执行器 / 预编译语句相关测试。
//
// 提供轻量 `Session`（共享 `Database`、可选事务快照、预编译 SELECT）、
// 行游标 `Cursor` 与请求优先级 `PriorityClient`；不连接真实存储。

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// 单元格取值：NULL / 整数 / 文本，供桩表行与查询结果使用。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
}
/// 一行：列名 → 值（有序映射，便于稳定比较）。
pub type Row = BTreeMap<String, Value>;

/// 内存表：列定义、行数据与 schema 版本（DDL 后递增以失效预编译计划）。
#[derive(Clone, Debug)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Row>,
    schema_version: u64,
}

/// 内存库：表集合与自增 `next_id`（插入时填充 `id` 列）。
#[derive(Clone, Debug, Default)]
struct Database {
    tables: HashMap<String, Table>,
    next_id: u64,
}

/// 预编译 SELECT 计划：投影列、可选等值谓词列、参数个数与绑定的 schema 版本。
#[derive(Clone, Debug)]
struct SelectPlan {
    table: String,
    projection: Vec<String>,
    predicate_column: Option<String>,
    parameter_count: usize,
    schema_version: u64,
}

/// 会话：共享库 + 可选事务副本 + 预编译语句表。
#[derive(Clone)]
pub struct Session {
    database: Arc<Mutex<Database>>,
    transaction: Option<Database>,
    prepared: HashMap<u64, SelectPlan>,
    next_statement_id: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new_shared(Arc::new(Mutex::new(Database::default())))
    }
}

impl Session {
    /// 基于共享 `Database` 构造会话（事务与 prepared 本地为空）。
    fn new_shared(database: Arc<Mutex<Database>>) -> Self {
        Self {
            database,
            transaction: None,
            prepared: HashMap::new(),
            next_statement_id: 1,
        }
    }

    /// 创建共享同一底层库的对等会话（模拟多连接）。
    pub fn peer(&self) -> Self {
        Self::new_shared(self.database.clone())
    }

    /// 只读访问：事务内读事务快照，否则读共享库。
    fn with_database<R>(&self, f: impl FnOnce(&Database) -> R) -> R {
        if let Some(transaction) = &self.transaction {
            f(transaction)
        } else {
            f(&self.database.lock().expect("database poisoned"))
        }
    }

    /// 可变访问：事务内改事务快照，否则改共享库。
    fn with_database_mut<R>(&mut self, f: impl FnOnce(&mut Database) -> R) -> R {
        if let Some(transaction) = &mut self.transaction {
            f(transaction)
        } else {
            f(&mut self.database.lock().expect("database poisoned"))
        }
    }

    /// 建表：至少一列，且表名不可重复。
    pub fn create_table(&mut self, name: &str, columns: &[&str]) -> Result<(), String> {
        if columns.is_empty() {
            return Err("table must have at least one column".into());
        }
        self.with_database_mut(|database| {
            if database.tables.contains_key(name) {
                return Err(format!("table {name} already exists"));
            }
            database.tables.insert(
                name.into(),
                Table {
                    columns: columns.iter().map(|column| (*column).into()).collect(),
                    rows: Vec::new(),
                    schema_version: 1,
                },
            );
            Ok(())
        })
    }

    /// 插入一行；未知列报错；缺省时自动写入自增 `id`。
    pub fn insert(&mut self, table: &str, mut row: Row) -> Result<u64, String> {
        let auto_id = if row.contains_key("id") {
            None
        } else if self.with_database(|database| {
            database
                .tables
                .get(table)
                .is_some_and(|table| table.columns.iter().any(|column| column == "id"))
        }) {
            // Auto IDs are allocated from the shared allocator even when the
            // insert is inside a transaction; rollback must not reuse them.
            let mut database = self.database.lock().expect("database poisoned");
            database.next_id += 1;
            Some(database.next_id)
        } else {
            None
        };
        self.with_database_mut(|database| {
            let table = database
                .tables
                .get_mut(table)
                .ok_or_else(|| "table not found".to_string())?;
            if row.keys().any(|column| !table.columns.contains(column)) {
                return Err("unknown column".into());
            }
            if let Some(id) = auto_id {
                row.insert("id".into(), Value::Int(id as i64));
                database.next_id = database.next_id.max(id);
            } else if let Some(Value::Int(id)) = row.get("id") {
                database.next_id = database.next_id.max(*id as u64);
            }
            let returned_id = auto_id.unwrap_or_else(|| {
                row.get("id")
                    .and_then(|value| match value {
                        Value::Int(id) => Some(*id as u64),
                        _ => None,
                    })
                    .unwrap_or(0)
            });
            table.rows.push(row);
            Ok(returned_id)
        })
    }

    /// 预编译 SELECT：校验投影/谓词列，绑定当前 schema 版本，返回语句 id。
    pub fn prepare_select(
        &mut self,
        table: &str,
        projection: &[&str],
        predicate_column: Option<&str>,
    ) -> Result<u64, String> {
        let plan = self.with_database(|database| {
            let table_data = database
                .tables
                .get(table)
                .ok_or_else(|| "table not found".to_string())?;
            for column in projection
                .iter()
                .copied()
                .chain(predicate_column.into_iter())
            {
                if !table_data
                    .columns
                    .iter()
                    .any(|candidate| candidate == column)
                {
                    return Err(format!("unknown column {column}"));
                }
            }
            Ok(SelectPlan {
                table: table.into(),
                projection: projection.iter().map(|column| (*column).into()).collect(),
                predicate_column: predicate_column.map(str::to_string),
                parameter_count: usize::from(predicate_column.is_some()),
                schema_version: table_data.schema_version,
            })
        })?;
        let id = self.next_statement_id;
        self.next_statement_id += 1;
        self.prepared.insert(id, plan);
        Ok(id)
    }

    /// 执行预编译计划：参数个数、schema 版本与投影列均需仍有效。
    pub fn execute_prepared(&self, id: u64, arguments: &[Value]) -> Result<Cursor, String> {
        let plan = self
            .prepared
            .get(&id)
            .ok_or_else(|| "prepared statement not found".to_string())?;
        if arguments.len() != plan.parameter_count {
            return Err("wrong parameter count".into());
        }
        self.with_database(|database| {
            let table = database
                .tables
                .get(&plan.table)
                .ok_or_else(|| "schema changed: table not found".to_string())?;
            // schema 变更或投影列消失 → 计划失效（模拟真实计划缓存失效）。
            if table.schema_version != plan.schema_version
                || plan
                    .projection
                    .iter()
                    .any(|column| !table.columns.contains(column))
            {
                return Err("prepared plan invalid after schema change".into());
            }
            let rows = table
                .rows
                .iter()
                .filter(|row| {
                    plan.predicate_column
                        .as_ref()
                        .is_none_or(|column| row.get(column) == arguments.first())
                })
                .map(|row| {
                    plan.projection
                        .iter()
                        .map(|column| row.get(column).cloned().unwrap_or(Value::Null))
                        .collect()
                })
                .collect();
            Ok(Cursor::new(rows))
        })
    }

    /// 释放预编译语句；不存在则报错。
    pub fn deallocate(&mut self, id: u64) -> Result<(), String> {
        self.prepared
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| "prepared statement not found".into())
    }

    /// Number of currently prepared statements, used to verify deallocation
    /// and plan-cache lifecycle semantics.
    pub fn prepared_len(&self) -> usize {
        self.prepared.len()
    }

    /// Drop a table and invalidate prepared statements referring to it.
    pub fn drop_table(&mut self, table: &str) -> Result<(), String> {
        self.with_database_mut(|database| {
            database
                .tables
                .remove(table)
                .map(|_| ())
                .ok_or_else(|| "table not found".to_string())
        })
    }

    /// Return the current table columns in declaration order.
    pub fn columns(&self, table: &str) -> Result<Vec<String>, String> {
        self.with_database(|database| {
            database
                .tables
                .get(table)
                .map(|table| table.columns.clone())
                .ok_or_else(|| "table not found".to_string())
        })
    }

    /// Return the number of rows in a table.
    pub fn row_count(&self, table: &str) -> Result<usize, String> {
        self.with_database(|database| {
            database
                .tables
                .get(table)
                .map(|table| table.rows.len())
                .ok_or_else(|| "table not found".to_string())
        })
    }

    /// 删列并递增 schema 版本，使依赖该列的预编译计划失效。
    pub fn drop_column(&mut self, table: &str, column: &str) -> Result<(), String> {
        self.with_database_mut(|database| {
            let table = database
                .tables
                .get_mut(table)
                .ok_or_else(|| "table not found".to_string())?;
            let position = table
                .columns
                .iter()
                .position(|candidate| candidate == column)
                .ok_or_else(|| "unknown column".to_string())?;
            table.columns.remove(position);
            for row in &mut table.rows {
                row.remove(column);
            }
            table.schema_version += 1;
            Ok(())
        })
    }

    /// 等值谓词更新：匹配 `predicate` 的行写入 `assignment`，返回影响行数。
    pub fn update_equal(
        &mut self,
        table: &str,
        predicate: (&str, Value),
        assignment: (&str, Value),
    ) -> Result<usize, String> {
        self.with_database_mut(|database| {
            let table = database
                .tables
                .get_mut(table)
                .ok_or_else(|| "table not found".to_string())?;
            if !table.columns.iter().any(|column| column == assignment.0) {
                return Err("unknown column".into());
            }
            if !table.columns.iter().any(|column| column == predicate.0) {
                return Err("unknown column".into());
            }
            let mut affected = 0;
            for row in &mut table.rows {
                if row.get(predicate.0) == Some(&predicate.1) {
                    row.insert(assignment.0.into(), assignment.1.clone());
                    affected += 1;
                }
            }
            Ok(affected)
        })
    }

    /// 等值谓词删除：去掉匹配行，返回删除行数。
    pub fn delete_equal(&mut self, table: &str, predicate: (&str, Value)) -> Result<usize, String> {
        self.with_database_mut(|database| {
            let table = database
                .tables
                .get_mut(table)
                .ok_or_else(|| "table not found".to_string())?;
            if !table.columns.iter().any(|column| column == predicate.0) {
                return Err("unknown column".into());
            }
            let before = table.rows.len();
            table
                .rows
                .retain(|row| row.get(predicate.0) != Some(&predicate.1));
            Ok(before - table.rows.len())
        })
    }

    /// 开启事务：克隆当前共享库作为会话私有快照；禁止嵌套。
    pub fn begin(&mut self) -> Result<(), String> {
        if self.transaction.is_some() {
            return Err("transaction already active".into());
        }
        self.transaction = Some(self.database.lock().expect("database poisoned").clone());
        Ok(())
    }

    /// 提交：用事务快照覆盖共享库。
    pub fn commit(&mut self) -> Result<(), String> {
        let transaction = self
            .transaction
            .take()
            .ok_or_else(|| "no transaction".to_string())?;
        *self.database.lock().expect("database poisoned") = transaction;
        Ok(())
    }

    /// 回滚：丢弃事务快照，共享库不变。
    pub fn rollback(&mut self) -> Result<(), String> {
        self.transaction
            .take()
            .map(|_| ())
            .ok_or_else(|| "no transaction".into())
    }
}

/// 结果游标：按行迭代，可注入一次性 next 错误，支持 close。
#[derive(Debug)]
pub struct Cursor {
    rows: Vec<Vec<Value>>,
    position: usize,
    closed: bool,
    fail_next: Option<String>,
}
impl Cursor {
    /// 由结果行集构造未关闭游标。
    pub fn new(rows: Vec<Vec<Value>>) -> Self {
        Self {
            rows,
            position: 0,
            closed: false,
            fail_next: None,
        }
    }
    /// 下次 `next` 返回指定错误（用于错误路径测试）。
    pub fn with_next_error(mut self, error: &str) -> Self {
        self.fail_next = Some(error.into());
        self
    }
    /// 取下一行；已关闭或注入错误时失败。
    pub fn next(&mut self) -> Result<Option<Vec<Value>>, String> {
        if self.closed {
            return Err("cursor closed".into());
        }
        if let Some(error) = self.fail_next.take() {
            return Err(error);
        }
        let row = self.rows.get(self.position).cloned();
        self.position += usize::from(row.is_some());
        Ok(row)
    }
    /// 关闭游标，后续 `next` 将失败。
    pub fn close(&mut self) {
        self.closed = true;
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// 请求优先级档位（低 / 普通 / 高）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Low,
    Normal,
    High,
}
/// 记录发送过的优先级序列，供调度相关测试断言。
#[derive(Debug, Default)]
pub struct PriorityClient {
    priorities: Vec<Priority>,
}
impl PriorityClient {
    /// 追加一次优先级发送记录。
    pub fn send(&mut self, priority: Priority) {
        self.priorities.push(priority);
    }
    /// 已记录的优先级序列。
    pub fn priorities(&self) -> &[Priority] {
        &self.priorities
    }
}

/// 由 `(列名, 值)` 切片构造一行。
pub fn row(values: &[(&str, Value)]) -> Row {
    values
        .iter()
        .map(|(column, value)| ((*column).into(), value.clone()))
        .collect()
}

/// Validate the maximum number of parameter markers accepted by the executor.
pub fn validate_parameter_count(count: usize) -> Result<(), String> {
    if count > u16::MAX as usize {
        Err("[executor:1390]Prepared statement contains too many placeholders".into())
    } else {
        Ok(())
    }
}

/// Model the planner guard used by the Cartesian-product regression.
pub fn validate_cartesian_product(allowed: bool) -> Result<(), String> {
    if allowed {
        Ok(())
    } else {
        Err("cartesian product is unsupported".into())
    }
}

/// 包级 TestMain / 共享会话烟测。
#[cfg(test)]
mod main_test;
/// 预编译语句生命周期与 schema 失效用例。
#[cfg(test)]
mod prepared_test;
/// 顺序执行器相关用例。
#[cfg(test)]
mod seq_executor_test;
