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

// LOCK STATS 执行器：锁定表或分区的统计信息，防止自动 ANALYZE 覆盖。
//
// 通过 InfoSchema（信息系统目录）解析表/分区 ID，再调用 StatsHandle
// 写入锁定元数据。单表且指定分区名时只锁分区；否则锁整表（含其全部分区）。

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// 本模块错误：携带可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(pub String);
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
/// 本模块 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 大小写不敏感标识符：`O` 保留原始拼写，`L` 为小写形式。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}
impl CIStr {
    /// 由任意字符串构造，自动生成小写副本。
    pub fn new(value: impl Into<String>) -> Self {
        let O = value.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

/// AST/计划中的表引用：库名、表名与可选分区名列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableName {
    pub Schema: CIStr,
    pub Name: CIStr,
    pub PartitionNames: Vec<CIStr>,
}
/// 分区定义：物理 ID 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub ID: i64,
    pub Name: CIStr,
}
/// 表元数据：表 ID 与可选分区列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableMeta {
    pub ID: i64,
    pub Partitions: Option<Vec<PartitionDefinition>>,
}

/// 信息系统目录接口：按库表名查找元数据。
pub trait InfoSchema: Send + Sync {
    fn TableByName(&self, schema: &str, table: &str) -> Result<TableMeta>;
}

/// 待锁定表的描述：全名与分区 ID→显示名映射。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatsLockTable {
    pub FullName: String,
    pub PartitionInfo: HashMap<i64, String>,
}

/// 统计子系统句柄：锁定/解锁表或分区级统计。
pub trait StatsHandle: Send + Sync {
    fn LockPartitions(
        &self,
        table_id: i64,
        table_name: &str,
        partitions: &HashMap<i64, String>,
    ) -> Result<String>;
    fn LockTables(&self, tables: &HashMap<i64, StatsLockTable>) -> Result<String>;
    fn RemoveLockedPartitions(
        &self,
        table_id: i64,
        table_name: &str,
        partitions: &HashMap<i64, String>,
    ) -> Result<String>;
    fn RemoveLockedTables(&self, tables: &HashMap<i64, StatsLockTable>) -> Result<String>;
}

/// 执行期依赖：StatsHandle、InfoSchema 与告警通道。
pub trait Runtime: Send + Sync {
    fn StatsHandle(&self) -> Option<Arc<dyn StatsHandle>>;
    fn InfoSchema(&self) -> Arc<dyn InfoSchema>;
    fn AppendWarning(&self, warning: Error);
}

/// LOCK STATS 算子：对指定表/分区写入统计锁。
pub struct LockExec {
    pub runtime: Arc<dyn Runtime>,
    pub Tables: Vec<TableName>,
}
impl LockExec {
    /// 打开算子。
    pub fn Open(&mut self) -> Result<()> {
        Ok(())
    }
    /// 关闭算子。
    pub fn Close(&mut self) -> Result<()> {
        Ok(())
    }
    /// 执行锁定：按是否仅锁分区选择 LockPartitions 或 LockTables。
    pub fn Next(&mut self) -> Result<()> {
        let handle = self
            .runtime
            .StatsHandle()
            .ok_or_else(|| Error("Lock Stats: handle is nil".into()))?;
        if self.Tables.is_empty() {
            return Err(Error("Lock Stats: table should not empty".into()));
        }
        let info_schema = self.runtime.InfoSchema();
        // 单表且带分区名 → 只锁这些分区；否则锁整表集合
        let message = if self.onlyLockPartitions() {
            let table = &self.Tables[0];
            let (table_id, partitions) =
                populatePartitionIDAndNames(table, &table.PartitionNames, info_schema.as_ref())?;
            handle.LockPartitions(
                table_id,
                &format!("{}.{}", table.Schema.L, table.Name.L),
                &partitions,
            )?
        } else {
            handle.LockTables(&populateTableAndPartitionIDs(
                &self.Tables,
                info_schema.as_ref(),
            )?)?
        };
        // 句柄返回的非空消息作为会话告警
        if !message.is_empty() {
            self.runtime.AppendWarning(Error(message));
        }
        Ok(())
    }
    /// 是否为“单表 + 显式分区列表”的分区级锁定模式。
    pub fn onlyLockPartitions(&self) -> bool {
        self.Tables.len() == 1 && !self.Tables[0].PartitionNames.is_empty()
    }
}

/// 将分区名解析为 (表 ID, 分区 ID→小写名) 映射。
pub fn populatePartitionIDAndNames(
    table: &TableName,
    partition_names: &[CIStr],
    info_schema: &dyn InfoSchema,
) -> Result<(i64, HashMap<i64, String>)> {
    if partition_names.is_empty() {
        return Err(Error("partition list should not be empty".into()));
    }
    let meta = info_schema.TableByName(&table.Schema.L, &table.Name.L)?;
    let definitions = meta.Partitions.as_ref().ok_or_else(|| {
        Error(format!(
            "table {}.{} is not a partition table",
            table.Schema.L, table.Name.L
        ))
    })?;
    let mut partitions = HashMap::with_capacity(partition_names.len());
    for name in partition_names {
        let definition = definitions
            .iter()
            // Go 的 FindPartitionByName 会先规范化查询名，再用 EqualFold
            // 比较元数据；即使 CIStr.L 尚未规范化，也必须大小写不敏感。
            .find(|partition| partition.Name.L.to_lowercase() == name.L.to_lowercase())
            .ok_or_else(|| Error(format!("unknown partition '{}'", name.O)))?;
        partitions.insert(definition.ID, name.L.clone());
    }
    Ok((meta.ID, partitions))
}

/// 为多张表构建锁定描述：整表 ID 映射，并填充分区显示名。
pub fn populateTableAndPartitionIDs(
    tables: &[TableName],
    info_schema: &dyn InfoSchema,
) -> Result<HashMap<i64, StatsLockTable>> {
    if tables.is_empty() {
        return Err(Error("table list should not be empty".into()));
    }
    let mut result = HashMap::with_capacity(tables.len());
    for table in tables {
        let meta = info_schema.TableByName(&table.Schema.L, &table.Name.L)?;
        let mut lock = StatsLockTable {
            FullName: format!("{}.{}", table.Schema.L, table.Name.L),
            PartitionInfo: HashMap::new(),
        };
        // 若为分区表，登记每个物理分区的完整显示名
        if let Some(partitions) = meta.Partitions {
            lock.PartitionInfo.reserve(partitions.len());
            for partition in partitions {
                lock.PartitionInfo
                    .insert(partition.ID, genFullPartitionName(table, &partition.Name.L));
            }
        }
        result.insert(meta.ID, lock);
    }
    Ok(result)
}

/// 生成分区的完整显示名，形如 `schema.table partition (p0)`。
pub fn genFullPartitionName(table: &TableName, partition_name: &str) -> String {
    format!(
        "{}.{} partition ({partition_name})",
        table.Schema.L, table.Name.L
    )
}
