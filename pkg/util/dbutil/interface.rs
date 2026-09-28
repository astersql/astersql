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

// 数据库执行器抽象：查询、写语句与事务接口，以及通用结果/错误类型。
//
// 对应 Go `pkg/util/dbutil` 中 `QueryExecutor`/`DBExecutor`。
// 当前可编译部分用 `Value`/`DbError`/`QueryResult` 表达驱动无关的 SQL 交互形状。

// check compatibility
// Go 通过空白赋值在编译期确认 *sql.DB 与 *sql.Conn 满足 DBExecutor。
// Rust 不接线 database/sql 类型，也不执行真实连接；这里保留兼容性检查的来源说明。
// var (
//     _ DBExecutor = &sql.DB{}
//     _ DBExecutor = &sql.Conn{}
// )

use std::fmt;

/// SQL 绑定参数与扫描结果的通用值类型（对应 Go `any`/驱动值的简化枚举）。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQL NULL。
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

/// 数据库错误：MySQL/TiDB 错误码、可选 SQLSTATE 与可读消息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DbError {
    /// MySQL 错误码（如 1213 死锁、1105 未知等）。
    pub code: u16,
    /// SQLSTATE 五字符状态码；未知时可为空。
    pub sql_state: Option<String>,
    /// 错误消息正文。
    pub message: String,
}
impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for DbError {}

/// 一次查询的列名与行数据；行内单元格为 `Value`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryResult {
    /// 结果集列名，顺序与每行列值对齐。
    pub columns: Vec<String>,
    /// 各行单元格值。
    pub rows: Vec<Vec<Value>>,
}

/// 只读查询执行器：带参数执行 SQL 并返回结果集。
pub trait QueryExecutor: Send + Sync {
    /// 执行查询，返回完整结果集。
    fn QueryContext(&self, query: &str, args: &[Value]) -> Result<QueryResult, DbError>;
    /// 执行查询并取首行；无行时返回与 Go `sql: no rows in result set` 对齐的错误。
    fn QueryRowContext(&self, query: &str, args: &[Value]) -> Result<Vec<Value>, DbError> {
        // 复用 QueryContext，再取第一行；空结果集映射为 no rows 错误。
        self.QueryContext(query, args)?
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: "sql: no rows in result set".to_owned(),
            })
    }
}

/// 事务句柄：在查询能力上增加 Exec、Commit、Rollback。
pub trait Transaction: QueryExecutor {
    /// 在事务内执行写语句，返回受影响行数。
    fn ExecContext(&mut self, query: &str, args: &[Value]) -> Result<u64, DbError>;
    /// 提交事务（消费 Box 所有权，对应 Go `Tx.Commit`）。
    fn Commit(self: Box<Self>) -> Result<(), DbError>;
    /// 回滚事务。
    fn Rollback(self: Box<Self>) -> Result<(), DbError>;
}

/// 读写执行器：在查询能力上支持开启事务与直接 Exec。
pub trait DBExecutor: QueryExecutor {
    /// 开启事务，返回动态分发的 `Transaction`。
    fn BeginTx(&self) -> Result<Box<dyn Transaction>, DbError>;
    /// 执行写语句，返回受影响行数。
    fn ExecContext(&self, query: &str, args: &[Value]) -> Result<u64, DbError>;
}
