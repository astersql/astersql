// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// BRIE 恢复路径的 DDL 辅助工具。
//
// 在 restore 时生成带 BR 注释的 CREATE DATABASE/TABLE，并批量建表；
// 遇 TiKV entry/transaction too large 时二分拆批，保持与 Go 相同的
// 左批先完成顺序。会话 query string 与外键检查通过 `BrieDDLContext` 注入。

#![allow(non_snake_case)]

use std::collections::BTreeMap;

/// CREATE TABLE SQL 字符串预分配容量。
pub const defaultCapOfCreateTable: usize = 512;
/// CREATE DATABASE SQL 字符串预分配容量。
pub const defaultCapOfCreateDatabase: usize = 64;

/// BRIE DDL 执行上下文：渲染 SQL、切换会话状态、建库建表。
pub trait BrieDDLContext {
    type Database: Clone;
    type Table: Clone;
    type CreateTableOption: Clone;
    type Error;

    /// 渲染 CREATE DATABASE 语句。
    fn render_create_database(&mut self, database: &Self::Database) -> Result<String, Self::Error>;
    /// 渲染 CREATE TABLE 语句。
    fn render_create_table(&mut self, table: &Self::Table) -> Result<String, Self::Error>;
    /// 确保库带有默认字符集。
    fn ensure_default_charset(&mut self, database: &mut Self::Database);
    /// 当前会话 query string。
    fn query_string(&self) -> Option<String>;
    /// 设置会话 query string（用于审计/展示）。
    fn set_query_string(&mut self, query: Option<String>);
    /// 当前是否开启外键检查。
    fn foreign_key_checks(&self) -> bool;
    /// 开关外键检查（建表时常临时关闭）。
    fn set_foreign_key_checks(&mut self, enabled: bool);
    /// 建库；已存在则报错。
    fn create_database_on_exist_error(
        &mut self,
        database: Self::Database,
    ) -> Result<(), Self::Error>;
    /// 建表；已存在则忽略。
    fn create_table_on_exist_ignore(
        &mut self,
        schema: &str,
        table: Self::Table,
        options: &[Self::CreateTableOption],
    ) -> Result<(), Self::Error>;
    /// 批量建表；已存在则忽略。
    fn batch_create_tables_on_exist_ignore(
        &mut self,
        schema: &str,
        tables: &[Self::Table],
        options: &[Self::CreateTableOption],
    ) -> Result<(), Self::Error>;
    /// 是否为 TiKV entry/transaction too large 类错误。
    fn is_entry_or_transaction_too_large(&self, error: &Self::Error) -> bool;
}

/// 生成带 BR 注释前缀的 CREATE DATABASE 语句文本。
pub fn showRestoredCreateDatabase<C: BrieDDLContext>(
    context: &mut C,
    database: &C::Database,
    br_comment: &str,
) -> Result<String, C::Error> {
    let mut result = String::with_capacity(defaultCapOfCreateDatabase);
    result.push_str(br_comment);
    result.push_str(&context.render_create_database(database)?);
    Ok(result)
}

/// 执行带注释的建库：临时写入 query string，恢复后返回。
pub fn BRIECreateDatabase<C: BrieDDLContext>(
    context: &mut C,
    database: &C::Database,
    br_comment: &str,
) -> Result<(), C::Error> {
    let query = showRestoredCreateDatabase(context, database, br_comment)?;
    // 保存并恢复会话 query string，避免污染调用方状态
    let previous_query = context.query_string();
    context.set_query_string(Some(query));

    let mut database = database.clone();
    context.ensure_default_charset(&mut database);
    let result = context.create_database_on_exist_error(database);
    context.set_query_string(previous_query);
    result
}

/// 生成带 BR 注释前缀的 CREATE TABLE 语句文本。
pub fn showRestoredCreateTable<C: BrieDDLContext>(
    context: &mut C,
    table: &C::Table,
    br_comment: &str,
) -> Result<String, C::Error> {
    let mut result = String::with_capacity(defaultCapOfCreateTable);
    result.push_str(br_comment);
    result.push_str(&context.render_create_table(table)?);
    Ok(result)
}

/// 执行带注释的建表：临时关闭外键检查并设置 query string。
pub fn BRIECreateTable<C: BrieDDLContext>(
    context: &mut C,
    schema: &str,
    table: &C::Table,
    br_comment: &str,
    options: &[C::CreateTableOption],
) -> Result<(), C::Error> {
    let query = showRestoredCreateTable(context, table, br_comment)?;
    let previous_query = context.query_string();
    // 建表期间关闭外键检查，结束后恢复
    let previous_foreign_key_checks = context.foreign_key_checks();
    context.set_query_string(Some(query));
    context.set_foreign_key_checks(false);

    let result = context.create_table_on_exist_ignore(schema, table.clone(), options);
    context.set_query_string(previous_query);
    context.set_foreign_key_checks(previous_foreign_key_checks);
    result
}

/// 按库批量建表；单批过大时由 `splitBatchCreateTable` 递归拆分。
pub fn BRIECreateTables<C: BrieDDLContext>(
    context: &mut C,
    cloned_tables: &BTreeMap<String, Vec<C::Table>>,
    br_comment: &str,
    options: &[C::CreateTableOption],
) -> Result<(), C::Error> {
    let previous_query = context.query_string();
    let previous_foreign_key_checks = context.foreign_key_checks();
    context.set_foreign_key_checks(false);

    // 关闭外键检查后按库组装查询并拆批创建
    let result = (|| {
        for (database, tables) in cloned_tables {
            let mut queries = Vec::with_capacity(tables.len());
            for table in tables {
                queries.push(showRestoredCreateTable(context, table, br_comment)?);
            }
            splitBatchCreateTable(context, database, tables, &queries, options)?;
        }
        Ok(())
    })();

    context.set_query_string(previous_query);
    context.set_foreign_key_checks(previous_foreign_key_checks);
    result
}

/// 将多条 SQL 用分号拼接成一批。
pub fn mergeQuerys(queries: &[String]) -> String {
    let mut result = String::new();
    for query in queries {
        result.push_str(query);
        result.push(';');
    }
    result
}

/// 仅对 TiKV entry/transaction-too-large 类错误二分拆批。
/// 左半批先完成再处理右半批，保持与 Go DDL 相同的顺序。
/// Splits only errors matching TiKV's entry/transaction-too-large errors. The
/// left batch is completed before the right batch, preserving Go DDL order.
pub fn splitBatchCreateTable<C: BrieDDLContext>(
    context: &mut C,
    schema: &str,
    tables: &[C::Table],
    queries: &[String],
    options: &[C::CreateTableOption],
) -> Result<(), C::Error> {
    // 先尝试整批；过大则对半递归
    assert_eq!(tables.len(), queries.len());
    context.set_query_string(Some(mergeQuerys(queries)));
    match context.batch_create_tables_on_exist_ignore(schema, tables, options) {
        Ok(()) => Ok(()),
        Err(error) if context.is_entry_or_transaction_too_large(&error) && tables.len() > 1 => {
            let middle = tables.len() / 2;
            splitBatchCreateTable(
                context,
                schema,
                &tables[..middle],
                &queries[..middle],
                options,
            )?;
            splitBatchCreateTable(
                context,
                schema,
                &tables[middle..],
                &queries[middle..],
                options,
            )
        }
        Err(error) => Err(error),
    }
}
