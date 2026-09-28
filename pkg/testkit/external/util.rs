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

// 用于查询表元数据的外部测试辅助工具。
//
// TiDB 从 `TestKit` 会话取得 domain，在需要时重新加载 InfoSchema，再执行查询。
// 本模块的 trait 构成这些组件的 Rust 边界：适配器让辅助工具不依赖具体会话实现，
// 同时保留原 Go 辅助函数的查询副作用与一致性语义。

#![allow(non_snake_case)]

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 外部测试辅助工具的错误，保留可供断言的错误消息。
pub struct ExternalError {
    message: String,
}

impl ExternalError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn index_not_found(database: &str, table: &str, index: &str) -> Self {
        Self::new(format!(
            "index {index} not found(db: {database}, tbl: {table})"
        ))
    }
}

impl fmt::Display for ExternalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExternalError {}

/// 外部测试辅助操作的统一结果类型。
pub type ExternalResult<T> = Result<T, ExternalError>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 测试查询所需的列元数据子集。
pub struct Column {
    /// 列名。
    pub name: String,
    /// 列的内部标识。
    pub id: i64,
    /// 列在表元数据中的位置。
    pub offset: usize,
    /// 是否为尚未公开或对用户隐藏的列。
    pub hidden: bool,
}

impl Column {
    pub fn new(name: impl Into<String>, id: i64, offset: usize, hidden: bool) -> Self {
        Self {
            name: name.into(),
            id,
            offset,
            hidden,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 测试查询所需的索引元数据子集。
pub struct Index {
    /// 索引名。
    pub name: String,
    /// 索引的内部标识。
    pub id: i64,
}

impl Index {
    pub fn new(name: impl Into<String>, id: i64) -> Self {
        Self {
            name: name.into(),
            id,
        }
    }
}

/// 下方三个辅助函数所需的表元数据操作。
pub trait TableMetadata {
    /// 返回公开列，对应 Go 的 `Table.Cols()`。
    fn columns(&self) -> &[Column];

    /// 返回全部物理列，对应 `TableCommon.Columns`，其中包括在线 DDL 变更期间
    /// 尚未进入公开 schema 的隐藏列。
    fn all_columns(&self) -> &[Column];

    /// 返回表上的索引元数据。
    fn indices(&self) -> &[Index];
}

/// 按库名和表名查询表元数据的 InfoSchema 抽象。
pub trait InfoSchema {
    type Table: TableMetadata;

    fn table_by_name(&self, database: &str, table: &str) -> ExternalResult<Self::Table>;
}

/// 为元数据查询提供 InfoSchema 及显式刷新能力的 domain 抽象。
pub trait Domain {
    type InfoSchema: InfoSchema;

    /// 在查询前强制刷新 schema，以保持 Go 辅助函数的一致性保证。
    fn reload(&self) -> ExternalResult<()>;
    fn info_schema(&self) -> Self::InfoSchema;
}

/// Go 中 `domain.GetDomain(tk.Session())` 操作的适配接口。
pub trait TestKitDomain {
    type Domain: Domain;

    fn domain(&self) -> &Self::Domain;
}

pub type DomainOf<T> = <T as TestKitDomain>::Domain;
pub type InfoSchemaOf<T> = <DomainOf<T> as Domain>::InfoSchema;
pub type TableOf<T> = <InfoSchemaOf<T> as InfoSchema>::Table;

/// 强制 domain 刷新 InfoSchema 后，按库名和表名取得表元数据。
pub fn GetTableByName<T>(test_kit: &T, database: &str, table: &str) -> ExternalResult<TableOf<T>>
where
    T: TestKitDomain,
{
    let domain = test_kit.domain();
    domain.reload()?;
    domain.info_schema().table_by_name(database, table)
}

/// 取得 `ALTER TABLE` 后发生变更的列，列名匹配不区分大小写。
///
/// `all_column` 为真时选择物理层的 `TableCommon.Columns` 视图，使测试能够观察
/// 尚未公开的过渡态列；否则只查询公开列。
pub fn GetModifyColumn<T>(
    test_kit: &T,
    database: &str,
    table: &str,
    column_name: &str,
    all_column: bool,
) -> ExternalResult<Option<Column>>
where
    T: TestKitDomain,
{
    let table = GetTableByName(test_kit, database, table)?;
    let wanted = column_name.to_lowercase();
    let columns = if all_column {
        table.all_columns()
    } else {
        table.columns()
    };
    Ok(columns
        .iter()
        .find(|column| column.name.to_lowercase() == wanted)
        .cloned())
}

/// 根据完整限定表名取得索引 ID。
///
/// Go 实现在索引不存在时调用 `require.FailNow`；这里返回错误，既保留相同的
/// 快速失败信息，也允许 Rust 测试框架使用 `?`、`expect` 或自定义断言。
pub fn GetIndexID<T>(
    test_kit: &T,
    database: &str,
    table: &str,
    index_name: &str,
) -> ExternalResult<i64>
where
    T: TestKitDomain,
{
    // 直接使用当前 InfoSchema，以对应 Go 的查询路径；与 `GetTableByName` 不同，
    // 原辅助函数在此处有意不刷新 domain。
    let table_metadata = test_kit
        .domain()
        .info_schema()
        .table_by_name(database, table)?;
    table_metadata
        .indices()
        .iter()
        .find(|index| index.name == index_name)
        .map(|index| index.id)
        .ok_or_else(|| ExternalError::index_not_found(database, table, index_name))
}
