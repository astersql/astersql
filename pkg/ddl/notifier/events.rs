// Copyright 2026 AsterSQL.
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

// Schema 变更事件（SchemaChangeEvent）的定义、构造与 JSON 编解码。
//
// DDL 通知器把一次表结构变更抽象为带 `ActionType` 的事件：可携带表/旧表、
// 增减分区、列、索引，以及删库时的精简库表信息。对外通过类型安全的
// `GetXxxInfo` / `NewXxxEvent` 访问；持久化时保留 Go JSON 契约中的完整模型字段。

use crate::{Error, ast, model};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Schema 变更事件的对外包装；内部可选持有 JSON 友好的载荷结构。
#[derive(Clone, Default)]
pub struct SchemaChangeEvent {
    pub(crate) inner: Option<JsonSchemaChangeEvent>,
}

/// 以 JSON 编解码结果比较相等，忽略内部指针差异。
impl PartialEq for SchemaChangeEvent {
    fn eq(&self, other: &Self) -> bool {
        self.MarshalJSON().ok() == other.MarshalJSON().ok()
    }
}
impl Eq for SchemaChangeEvent {}
impl fmt::Debug for SchemaChangeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SchemaChangeEvent")
            .field(&self.String())
            .finish()
    }
}
impl fmt::Display for SchemaChangeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}

/// 将 ActionType 映射为人类可读的事件类型名，供 `String()` 输出。
fn action_name(tp: model::ActionType) -> &'static str {
    model::group_3::action_type_string(tp)
}

impl SchemaChangeEvent {
    /// 拼出包含事件类型、表/分区/列/索引等关键字段的调试字符串。
    pub fn String(&self) -> String {
        // 按固定顺序追加可选字段，保证测试断言的字符串格式稳定。
        let Some(inner) = self.inner.as_ref() else {
            return "nil SchemaChangeEvent".to_owned();
        };
        let mut result = format!("(Event Type: {}", action_name(inner.Tp));
        if let Some(table) = &inner.TableInfo {
            result.push_str(&format!(
                ", Table ID: {}, Table Name: {}",
                table.ID, table.Name.O
            ));
        }
        if let Some(table) = &inner.OldTableInfo {
            result.push_str(&format!(
                ", Old Table ID: {}, Old Table Name: {}",
                table.ID, table.Name.O
            ));
        }
        if inner.OldTableID4Partition != 0 {
            result.push_str(&format!(
                ", Old Table ID for Partition: {}",
                inner.OldTableID4Partition
            ));
        }
        if let Some(partitions) = &inner.AddedPartInfo {
            for partition in &partitions.Definitions {
                if !partition.Name.L.is_empty() {
                    result.push_str(&format!(", Partition Name: {}", partition.Name.O));
                }
                result.push_str(&format!(", Partition ID: {}", partition.ID));
            }
        }
        if let Some(partitions) = &inner.DroppedPartInfo {
            for partition in &partitions.Definitions {
                if !partition.Name.L.is_empty() {
                    result.push_str(&format!(", Dropped Partition Name: {}", partition.Name.O));
                }
                result.push_str(&format!(", Dropped Partition ID: {}", partition.ID));
            }
        }
        for column in &inner.Columns {
            result.push_str(&format!(
                ", Column ID: {}, Column Name: {}",
                column.ID, column.Name.O
            ));
        }
        for index in &inner.Indexes {
            result.push_str(&format!(
                ", Index ID: {}, Index Name: {}",
                index.ID, index.Name.O
            ));
        }
        result.push(')');
        result
    }

    /// 返回事件的 ActionType；空事件视为 ActionNone。
    pub fn GetType(&self) -> model::ActionType {
        self.inner
            .as_ref()
            .map_or(model::ActionNone, |inner| inner.Tp)
    }

    /// 断言事件类型匹配后返回内部载荷；类型不符则 panic。
    fn expect(&self, expected: model::ActionType) -> &JsonSchemaChangeEvent {
        let inner = self.inner.as_ref().expect("nil SchemaChangeEvent");
        assert_eq!(inner.Tp, expected, "unexpected schema change event type");
        inner
    }

    /// 取建表事件中的新表信息。
    pub fn GetCreateTableInfo(&self) -> Option<Box<model::TableInfo>> {
        self.expect(model::ACTION_CREATE_TABLE).TableInfo.clone()
    }
    /// 取截断表事件：(新表, 旧表)。
    pub fn GetTruncateTableInfo(
        &self,
    ) -> (Option<Box<model::TableInfo>>, Option<Box<model::TableInfo>>) {
        let inner = self.expect(model::ACTION_TRUNCATE_TABLE);
        (inner.TableInfo.clone(), inner.OldTableInfo.clone())
    }
    /// 取删表事件中的旧表信息。
    pub fn GetDropTableInfo(&self) -> Option<Box<model::TableInfo>> {
        self.expect(model::ACTION_DROP_TABLE).OldTableInfo.clone()
    }
    /// 取加列事件：(所属表, 新增列列表)。
    pub fn GetAddColumnInfo(&self) -> (Option<Box<model::TableInfo>>, Vec<Box<model::ColumnInfo>>) {
        let inner = self.expect(model::ACTION_ADD_COLUMN);
        (inner.TableInfo.clone(), inner.Columns.clone())
    }
    /// 取改列事件：(所属表, 列列表, 是否已 analyze)。
    pub fn GetModifyColumnInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Vec<Box<model::ColumnInfo>>,
        bool,
    ) {
        let inner = self.expect(model::ACTION_MODIFY_COLUMN);
        (
            inner.TableInfo.clone(),
            inner.Columns.clone(),
            inner.Analyzed,
        )
    }
    /// 取加分区事件：(表, 新增分区信息)。
    pub fn GetAddPartitionInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_ADD_TABLE_PARTITION);
        (inner.TableInfo.clone(), inner.AddedPartInfo.clone())
    }
    /// 取截断分区事件：(表, 新增分区, 被截断/删除分区)。
    pub fn GetTruncatePartitionInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_TRUNCATE_TABLE_PARTITION);
        (
            inner.TableInfo.clone(),
            inner.AddedPartInfo.clone(),
            inner.DroppedPartInfo.clone(),
        )
    }
    /// 取删分区事件：(表, 被删分区信息)。
    pub fn GetDropPartitionInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_DROP_TABLE_PARTITION);
        (inner.TableInfo.clone(), inner.DroppedPartInfo.clone())
    }
    /// 取交换分区事件：(分区表, 参与交换的分区, 普通表)。
    pub fn GetExchangePartitionInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
        Option<Box<model::TableInfo>>,
    ) {
        let inner = self.expect(model::ACTION_EXCHANGE_TABLE_PARTITION);
        (
            inner.TableInfo.clone(),
            inner.AddedPartInfo.clone(),
            inner.OldTableInfo.clone(),
        )
    }
    /// 取重组分区事件：(表, 新增分区, 被替换分区)。
    pub fn GetReorganizePartitionInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_REORGANIZE_PARTITION);
        (
            inner.TableInfo.clone(),
            inner.AddedPartInfo.clone(),
            inner.DroppedPartInfo.clone(),
        )
    }
    /// 取改为分区表事件：(旧表 ID, 新表, 新增分区定义)。
    pub fn GetAddPartitioningInfo(
        &self,
    ) -> (
        i64,
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_ALTER_TABLE_PARTITIONING);
        (
            inner.OldTableID4Partition,
            inner.TableInfo.clone(),
            inner.AddedPartInfo.clone(),
        )
    }
    pub fn GetRemovePartitioningInfo(
        // 取移除分区事件：(旧分区表 ID, 新普通表, 被移除分区)。
        &self,
    ) -> (
        i64,
        Option<Box<model::TableInfo>>,
        Option<Box<model::PartitionInfo>>,
    ) {
        let inner = self.expect(model::ACTION_REMOVE_PARTITIONING);
        (
            inner.OldTableID4Partition,
            inner.TableInfo.clone(),
            inner.DroppedPartInfo.clone(),
        )
    }
    /// 取加索引事件：(表, 索引列表, 是否已 analyze)。
    pub fn GetAddIndexInfo(
        &self,
    ) -> (
        Option<Box<model::TableInfo>>,
        Vec<Box<model::IndexInfo>>,
        bool,
    ) {
        let inner = self.expect(model::ACTION_ADD_INDEX);
        (
            inner.TableInfo.clone(),
            inner.Indexes.clone(),
            inner.Analyzed,
        )
    }
    /// 取删库事件的精简库信息。
    pub fn GetDropSchemaInfo(&self) -> Option<Box<MiniDBInfoForSchemaEvent>> {
        self.expect(model::ACTION_DROP_SCHEMA).MiniDBInfo.clone()
    }

    /// 将事件序列化为 JSON 字节（Wire 格式）。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&self.inner.as_ref().map(WireEvent::from)).map_err(Error::from)
    }

    /// 从 JSON 字节反序列化并覆盖当前事件。
    pub fn UnmarshalJSON(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let decoded: Option<WireEvent> = serde_json::from_slice(bytes)?;
        self.inner = Some(decoded.map_or_else(JsonSchemaChangeEvent::default, Into::into));
        Ok(())
    }

    /// 用 source 中“有值”的字段覆盖 self，供 List 复用缓冲区时合并残留。
    pub(crate) fn overwrite_from(&mut self, source: &SchemaChangeEvent) {
        let Some(source) = source.inner.as_ref() else {
            self.inner = None;
            return;
        };
        // 仅覆盖 source 已设置的字段，避免清空调用方缓冲区中的其它残留。
        if let Some(target) = self.inner.as_mut() {
            if source.MiniDBInfo.is_some() {
                target.MiniDBInfo = source.MiniDBInfo.clone();
            }
            if source.TableInfo.is_some() {
                target.TableInfo = source.TableInfo.clone();
            }
            if source.OldTableInfo.is_some() {
                target.OldTableInfo = source.OldTableInfo.clone();
            }
            if source.AddedPartInfo.is_some() {
                target.AddedPartInfo = source.AddedPartInfo.clone();
            }
            if source.DroppedPartInfo.is_some() {
                target.DroppedPartInfo = source.DroppedPartInfo.clone();
            }
            if !source.Columns.is_empty() {
                target.Columns = source.Columns.clone();
            }
            if !source.Indexes.is_empty() {
                target.Indexes = source.Indexes.clone();
            }
            target.Analyzed = source.Analyzed;
            target.OldTableID4Partition = source.OldTableID4Partition;
            target.Tp = source.Tp;
        } else {
            self.inner = Some(source.clone());
        }
    }
}

/// 生成简单构造函数：设置 ActionType 与若干命名字段，其余走 Default。
macro_rules! event_constructor {
    ($name:ident, $action:expr, $($field:ident : $value:ident : $ty:ty),* $(,)?) => {
        pub fn $name($($value: $ty),*) -> SchemaChangeEvent {
            SchemaChangeEvent { inner: Some(JsonSchemaChangeEvent { Tp: $action, $($field: $value,)* ..Default::default() }) }
        }
    };
}

event_constructor!(NewCreateTableEvent, model::ACTION_CREATE_TABLE, TableInfo: table: Option<Box<model::TableInfo>>);
event_constructor!(NewTruncateTableEvent, model::ACTION_TRUNCATE_TABLE, TableInfo: table: Option<Box<model::TableInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);
event_constructor!(NewDropTableEvent, model::ACTION_DROP_TABLE, OldTableInfo: table: Option<Box<model::TableInfo>>);
event_constructor!(NewAddColumnEvent, model::ACTION_ADD_COLUMN, TableInfo: table: Option<Box<model::TableInfo>>, Columns: columns: Vec<Box<model::ColumnInfo>>);

/// 构造修改列事件；`analyzed` 表示是否已完成相关统计信息收集。
pub fn NewModifyColumnEvent(
    table: Option<Box<model::TableInfo>>,
    columns: Vec<Box<model::ColumnInfo>>,
    analyzed: bool,
) -> SchemaChangeEvent {
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_MODIFY_COLUMN,
            TableInfo: table,
            Columns: columns,
            Analyzed: analyzed,
            ..Default::default()
        }),
    }
}
event_constructor!(NewAddPartitionEvent, model::ACTION_ADD_TABLE_PARTITION, TableInfo: table: Option<Box<model::TableInfo>>, AddedPartInfo: added: Option<Box<model::PartitionInfo>>);
event_constructor!(NewTruncatePartitionEvent, model::ACTION_TRUNCATE_TABLE_PARTITION, TableInfo: table: Option<Box<model::TableInfo>>, AddedPartInfo: added: Option<Box<model::PartitionInfo>>, DroppedPartInfo: dropped: Option<Box<model::PartitionInfo>>);
event_constructor!(NewDropPartitionEvent, model::ACTION_DROP_TABLE_PARTITION, TableInfo: table: Option<Box<model::TableInfo>>, DroppedPartInfo: dropped: Option<Box<model::PartitionInfo>>);
event_constructor!(NewExchangePartitionEvent, model::ACTION_EXCHANGE_TABLE_PARTITION, TableInfo: table: Option<Box<model::TableInfo>>, AddedPartInfo: partition: Option<Box<model::PartitionInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);
event_constructor!(NewReorganizePartitionEvent, model::ACTION_REORGANIZE_PARTITION, TableInfo: table: Option<Box<model::TableInfo>>, AddedPartInfo: added: Option<Box<model::PartitionInfo>>, DroppedPartInfo: dropped: Option<Box<model::PartitionInfo>>);

/// 构造“改为分区表”事件，记录转换前的旧表 ID。
pub fn NewAddPartitioningEvent(
    old_table_id: i64,
    table: Option<Box<model::TableInfo>>,
    added: Option<Box<model::PartitionInfo>>,
) -> SchemaChangeEvent {
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_ALTER_TABLE_PARTITIONING,
            OldTableID4Partition: old_table_id,
            TableInfo: table,
            AddedPartInfo: added,
            ..Default::default()
        }),
    }
}
/// 构造“移除分区”事件，记录转换前的旧分区表 ID。
pub fn NewRemovePartitioningEvent(
    old_table_id: i64,
    table: Option<Box<model::TableInfo>>,
    dropped: Option<Box<model::PartitionInfo>>,
) -> SchemaChangeEvent {
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_REMOVE_PARTITIONING,
            OldTableID4Partition: old_table_id,
            TableInfo: table,
            DroppedPartInfo: dropped,
            ..Default::default()
        }),
    }
}
/// 构造加索引事件；`analyzed` 表示索引相关统计是否已分析。
pub fn NewAddIndexEvent(
    table: Option<Box<model::TableInfo>>,
    indexes: Vec<Box<model::IndexInfo>>,
    analyzed: bool,
) -> SchemaChangeEvent {
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_ADD_INDEX,
            TableInfo: table,
            Indexes: indexes,
            Analyzed: analyzed,
            ..Default::default()
        }),
    }
}
/// 构造集群闪回（flashback cluster）事件，无额外载荷。
pub fn NewFlashbackClusterEvent() -> SchemaChangeEvent {
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_FLASHBACK_CLUSTER,
            ..Default::default()
        }),
    }
}
/// 构造删库事件：把库与其下表/分区压缩为 Mini* 结构写入事件。
pub fn NewDropSchemaEvent(
    db: &model::DBInfo,
    tables: Vec<Box<model::TableInfo>>,
) -> SchemaChangeEvent {
    // 只保留 ID/名称/分区列表，降低通知通道上的元数据体积。
    let tables = tables
        .into_iter()
        .map(|table| {
            Box::new(MiniTableInfoForSchemaEvent {
                ID: table.ID,
                Name: table.Name.clone(),
                Partitions: table.Partition.as_ref().map_or_else(Vec::new, |partition| {
                    partition
                        .Definitions
                        .iter()
                        .map(|definition| {
                            Box::new(MiniPartitionInfoForSchemaEvent {
                                ID: definition.ID,
                                Name: definition.Name.clone(),
                            })
                        })
                        .collect()
                }),
            })
        })
        .collect();
    SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_DROP_SCHEMA,
            MiniDBInfo: Some(Box::new(MiniDBInfoForSchemaEvent {
                ID: db.ID,
                Name: db.Name.clone(),
                Tables: tables,
            })),
            ..Default::default()
        }),
    }
}

/// 删库事件中的精简库信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MiniDBInfoForSchemaEvent {
    /// 库 ID。
    pub ID: i64,
    /// 库名（大小写不敏感标识）。
    pub Name: ast::CIStr,
    /// 库下表的精简列表。
    pub Tables: Vec<Box<MiniTableInfoForSchemaEvent>>,
}
/// 删库事件中的精简表信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MiniTableInfoForSchemaEvent {
    /// 表 ID。
    pub ID: i64,
    /// 表名。
    pub Name: ast::CIStr,
    /// 表下分区的精简列表。
    pub Partitions: Vec<Box<MiniPartitionInfoForSchemaEvent>>,
}
/// 删库事件中的精简分区信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MiniPartitionInfoForSchemaEvent {
    /// 分区 ID。
    pub ID: i64,
    /// 分区名。
    pub Name: ast::CIStr,
}

/// 事件内部载荷：完整 TableInfo/PartitionInfo 与 ActionType 等字段。
#[derive(Clone, Default)]
pub(crate) struct JsonSchemaChangeEvent {
    pub MiniDBInfo: Option<Box<MiniDBInfoForSchemaEvent>>,
    pub TableInfo: Option<Box<model::TableInfo>>,
    pub OldTableInfo: Option<Box<model::TableInfo>>,
    pub AddedPartInfo: Option<Box<model::PartitionInfo>>,
    pub DroppedPartInfo: Option<Box<model::PartitionInfo>>,
    pub Columns: Vec<Box<model::ColumnInfo>>,
    pub Indexes: Vec<Box<model::IndexInfo>>,
    pub Analyzed: bool,
    pub OldTableID4Partition: i64,
    pub Tp: model::ActionType,
}

/// 精简分区的线格式。
#[derive(Serialize, Deserialize)]
struct MiniPartitionWire {
    id: i64,
    name: ast::CIStr,
}
/// 精简表的线格式。
#[derive(Serialize, Deserialize)]
struct MiniTableWire {
    id: i64,
    name: ast::CIStr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    partitions: Vec<MiniPartitionWire>,
}
/// 精简库的线格式。
#[derive(Serialize, Deserialize)]
struct MiniDbWire {
    id: i64,
    name: ast::CIStr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tables: Vec<MiniTableWire>,
}
/// Schema 变更事件的 serde 线格式，字段按需省略默认值。
#[derive(Serialize, Deserialize)]
struct WireEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    mini_db_info: Option<MiniDbWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    table_info: Option<Box<model::TableInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_table_info: Option<Box<model::TableInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added_partition_info: Option<Box<model::PartitionInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped_partition_info: Option<Box<model::PartitionInfo>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    columns: Vec<Box<model::ColumnInfo>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    indexes: Vec<Box<model::IndexInfo>>,
    #[serde(
        rename = "Analyzed",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    analyzed: bool,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    old_table_id_for_partition: i64,
    #[serde(rename = "type", default, skip_serializing_if = "is_zero_u8")]
    tp: model::ActionType,
}
/// serde 辅助：i64 为 0 时跳过序列化。
fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
/// serde 辅助：u8 为 0 时跳过序列化。
fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

impl From<&JsonSchemaChangeEvent> for WireEvent {
    fn from(value: &JsonSchemaChangeEvent) -> Self {
        Self {
            mini_db_info: value.MiniDBInfo.as_deref().map(|db| MiniDbWire {
                id: db.ID,
                name: db.Name.clone(),
                tables: db
                    .Tables
                    .iter()
                    .map(|table| MiniTableWire {
                        id: table.ID,
                        name: table.Name.clone(),
                        partitions: table
                            .Partitions
                            .iter()
                            .map(|partition| MiniPartitionWire {
                                id: partition.ID,
                                name: partition.Name.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            table_info: value.TableInfo.clone(),
            old_table_info: value.OldTableInfo.clone(),
            added_partition_info: value.AddedPartInfo.clone(),
            dropped_partition_info: value.DroppedPartInfo.clone(),
            columns: value.Columns.clone(),
            indexes: value.Indexes.clone(),
            analyzed: value.Analyzed,
            old_table_id_for_partition: value.OldTableID4Partition,
            tp: value.Tp,
        }
    }
}
impl From<WireEvent> for JsonSchemaChangeEvent {
    fn from(value: WireEvent) -> Self {
        Self {
            MiniDBInfo: value.mini_db_info.map(|db| {
                Box::new(MiniDBInfoForSchemaEvent {
                    ID: db.id,
                    Name: db.name,
                    Tables: db
                        .tables
                        .into_iter()
                        .map(|table| {
                            Box::new(MiniTableInfoForSchemaEvent {
                                ID: table.id,
                                Name: table.name,
                                Partitions: table
                                    .partitions
                                    .into_iter()
                                    .map(|partition| {
                                        Box::new(MiniPartitionInfoForSchemaEvent {
                                            ID: partition.id,
                                            Name: partition.name,
                                        })
                                    })
                                    .collect(),
                            })
                        })
                        .collect(),
                })
            }),
            TableInfo: value.table_info,
            OldTableInfo: value.old_table_info,
            AddedPartInfo: value.added_partition_info,
            DroppedPartInfo: value.dropped_partition_info,
            Columns: value.columns,
            Indexes: value.indexes,
            Analyzed: value.analyzed,
            OldTableID4Partition: value.old_table_id_for_partition,
            Tp: value.tp,
        }
    }
}

// Go events.go materialized-view alter events retain both complete tables.
event_constructor!(NewAlterMaterializedViewRefreshEvent, model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_REFRESH, TableInfo: table: Option<Box<model::TableInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);
event_constructor!(NewAlterMaterializedViewAttributesEvent, model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES, TableInfo: table: Option<Box<model::TableInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);
event_constructor!(NewAlterMaterializedViewLogPurgeEvent, model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE, TableInfo: table: Option<Box<model::TableInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);
event_constructor!(NewMViewRefreshOutOfPlaceCutoverEvent, model::group_3::ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER, TableInfo: table: Option<Box<model::TableInfo>>, OldTableInfo: old: Option<Box<model::TableInfo>>);

impl SchemaChangeEvent {
    pub fn GetMViewRefreshOutOfPlaceCutoverInfo(
        &self,
    ) -> (Option<Box<model::TableInfo>>, Option<Box<model::TableInfo>>) {
        let inner = self.expect(model::group_3::ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER);
        (inner.TableInfo.clone(), inner.OldTableInfo.clone())
    }
    /// Go GetAlterMaterializedViewRefreshInfo returns the new complete table.
    pub fn GetAlterMaterializedViewRefreshInfo(&self) -> Option<Box<model::TableInfo>> {
        self.expect(model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_REFRESH)
            .TableInfo
            .clone()
    }

    /// Go GetAlterMaterializedViewAttributesInfo returns the new complete table.
    pub fn GetAlterMaterializedViewAttributesInfo(&self) -> Option<Box<model::TableInfo>> {
        self.expect(model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES)
            .TableInfo
            .clone()
    }

    /// Go GetAlterMaterializedViewLogPurgeInfo returns the new complete table.
    pub fn GetAlterMaterializedViewLogPurgeInfo(&self) -> Option<Box<model::TableInfo>> {
        self.expect(model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE)
            .TableInfo
            .clone()
    }
}
