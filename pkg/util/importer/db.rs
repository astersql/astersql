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

// 导入用数据库抽象与行数据生成。
//
// 定义 `Database`/`DatabaseTransaction`/`DatabaseConnector` 以便注入真实驱动或测试桩；
// `generate_column_data` / `generate_row_data` 按列类型与唯一性约束生成 INSERT SQL。

use crate::config::{DbConfig, ImporterError};
use crate::parser::{Column, FieldKind, Table};
use crate::rand::{
    rand_date, rand_i64, rand_string, rand_time, rand_timestamp, rand_usize, rand_year,
};
use std::sync::Arc;

/// 事务句柄：执行 SQL 并提交。
pub trait DatabaseTransaction: Send {
    fn execute(&mut self, sql: &str) -> Result<(), ImporterError>;
    fn commit(self: Box<Self>) -> Result<(), ImporterError>;
}

/// 数据库连接：执行语句、开启事务、关闭。
pub trait Database: Send + Sync {
    fn execute(&self, sql: &str) -> Result<(), ImporterError>;
    fn begin(&self) -> Result<Box<dyn DatabaseTransaction>, ImporterError>;
    fn close(&self) -> Result<(), ImporterError>;
}

/// 按 DSN 打开数据库连接的工厂。
pub trait DatabaseConnector: Send + Sync {
    fn open(&self, dsn: &str) -> Result<Arc<dyn Database>, ImporterError>;
}

/// 解析列注释中的 range，得到整型取值上下界。
fn integer_range(column: &Column, minimum: i64, maximum: i64) -> Result<(i64, i64), ImporterError> {
    if column.minimum.is_empty() {
        return Ok((minimum, maximum));
    }
    let minimum = column
        .minimum
        .parse()
        .map_err(|_| ImporterError::InvalidRange(column.minimum.clone()))?;
    let maximum = if column.maximum.is_empty() {
        maximum
    } else {
        column
            .maximum
            .parse()
            .map_err(|_| ImporterError::InvalidRange(column.maximum.clone()))?
    };
    Ok((minimum, maximum))
}

/// 非唯一整型：优先从 `set` 枚举取值，否则在区间内随机。
fn random_integer(column: &Column, minimum: i64, maximum: i64) -> Result<i64, ImporterError> {
    if !column.set.is_empty() {
        let index = rand_usize(0, column.set.len() - 1)?;
        return column.set[index]
            .parse()
            .map_err(|_| ImporterError::InvalidRange(column.set[index].clone()));
    }
    let (minimum, maximum) = integer_range(column, minimum, maximum)?;
    rand_i64(minimum, maximum)
}

/// 唯一整型：初始化 `Datum` 后取下一个唯一值。
fn unique_integer(column: &Column, minimum: i64, maximum: i64) -> Result<i64, ImporterError> {
    let (minimum, maximum) = integer_range(column, minimum, maximum)?;
    column
        .data
        .set_init_int64_value(column.step, minimum, maximum);
    Ok(column.data.unique_i64())
}

/// 按列类型与表唯一索引信息生成单个字段的 SQL 字面量。
pub fn generate_column_data(table: &Table, column: &Column) -> Result<String, ImporterError> {
    let unique = table.unique_indices.contains(&column.name);
    let unsigned = column.field_type.unsigned;
    // 整型列：唯一走 Datum；无符号随机从 0 起；有符号用类型下界。
    let integer = |signed_minimum, signed_maximum, unique_maximum| {
        if unique {
            unique_integer(column, 0, unique_maximum)
        } else if unsigned {
            random_integer(column, 0, unique_maximum)
        } else {
            random_integer(column, signed_minimum, signed_maximum)
        }
    };
    match column.field_type.kind {
        FieldKind::TinyInt => {
            integer(i8::MIN as i64, i8::MAX as i64, u8::MAX as i64).map(|v| v.to_string())
        }
        FieldKind::SmallInt => {
            integer(i16::MIN as i64, i16::MAX as i64, u16::MAX as i64).map(|v| v.to_string())
        }
        FieldKind::Int => {
            integer(i32::MIN as i64, i32::MAX as i64, u32::MAX as i64).map(|v| v.to_string())
        }
        FieldKind::BigInt => {
            integer(i32::MIN as i64, i32::MAX as i64, i64::MAX).map(|v| v.to_string())
        }
        FieldKind::Varchar | FieldKind::String | FieldKind::Blob => {
            let value = if unique {
                column.data.unique_string(column.field_type.length)
            } else {
                rand_string(rand_usize(1, column.field_type.length.max(1))?)
            };
            Ok(format!("'{value}'"))
        }
        FieldKind::Float | FieldKind::Double | FieldKind::Decimal => {
            integer(i32::MIN as i64, i32::MAX as i64, i64::MAX)
                .map(|value| (value as f64).to_string())
        }
        FieldKind::Date => Ok(format!(
            "'{}'",
            if unique {
                column.data.unique_date()
            } else {
                rand_date(&column.minimum, &column.maximum)?
            }
        )),
        FieldKind::DateTime | FieldKind::Timestamp => Ok(format!(
            "'{}'",
            if unique {
                column.data.unique_timestamp()
            } else {
                rand_timestamp(&column.minimum, &column.maximum)?
            }
        )),
        FieldKind::Time => Ok(format!(
            "'{}'",
            if unique {
                column.data.unique_time()
            } else {
                rand_time(&column.minimum, &column.maximum)?
            }
        )),
        FieldKind::Year => Ok(format!(
            "'{}'",
            if unique {
                column.data.unique_year()
            } else {
                rand_year(&column.minimum, &column.maximum)?
            }
        )),
    }
}

/// 生成单行 `INSERT INTO ... VALUES (...);`。
pub fn generate_row_data(table: &Table) -> Result<String, ImporterError> {
    let values = table
        .columns
        .iter()
        .map(|column| generate_column_data(table, column))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!(
        "insert into {} ({}) values ({});",
        table.name,
        table.column_list,
        values.join(",")
    ))
}

/// 连续生成 `count` 条 INSERT 语句。
pub fn generate_row_data_batch(table: &Table, count: usize) -> Result<Vec<String>, ImporterError> {
    (0..count).map(|_| generate_row_data(table)).collect()
}

/// 空 SQL 直接成功；否则委托给 `Database::execute`。
pub fn execute_sql(database: &dyn Database, sql: &str) -> Result<(), ImporterError> {
    if sql.is_empty() {
        Ok(())
    } else {
        database.execute(sql)
    }
}

/// 按 DSN 打开 `count` 个连接，供多 worker 并行使用。
pub fn create_databases(
    connector: &dyn DatabaseConnector,
    config: &DbConfig,
    count: usize,
) -> Result<Vec<Arc<dyn Database>>, ImporterError> {
    let mut databases = Vec::with_capacity(count);
    for _ in 0..count {
        match connector.open(&config.dsn()) {
            Ok(database) => databases.push(database),
            Err(error) => {
                close_databases(&databases);
                return Err(error);
            }
        }
    }
    Ok(databases)
}

/// 关闭全部连接，收集关闭阶段错误（不中断后续关闭）。
pub fn close_databases(databases: &[Arc<dyn Database>]) -> Vec<ImporterError> {
    databases
        .iter()
        .filter_map(|database| database.close().err())
        .collect()
}
