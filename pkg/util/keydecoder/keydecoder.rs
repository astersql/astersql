// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TiDB 键诊断解码：将行键/索引键还原为可读的库表分区与 handle/索引信息。
//
// 用于排查与诊断输出（JSON）：通过 infoschema（信息模式，表元数据视图）
// 把物理 table/partition id 映射到库名、表名、分区名与索引名。
// Handle 是行标识：整数 Handle、公共 Handle（联合主键编码）或分区 Handle。

#![allow(non_snake_case, non_upper_case_globals)]

use std::fmt;

use astersql_infoschema as infoschema;
use astersql_tablecodec as tablecodec;
use astersql_tablecodec::kv;
use astersql_util_logutil as logutil;

/// Handle 类型标签字符串（与诊断 JSON 字段 `handle_type` 对齐）。
pub type HandleType = &'static str;

/// 整数 Handle（自增主键等）。
pub const IntHandle: HandleType = "int";
/// 公共 Handle（非整数主键编码）。
pub const CommonHandle: HandleType = "common";
/// 未能识别的 Handle 类型。
pub const UnknownHandle: HandleType = "unknown";

/// 解码后的键诊断结果，字段名与 Go JSON 序列化约定一致；空值在序列化时省略。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct DecodedKey {
    #[serde(rename = "db_name", skip_serializing_if = "String::is_empty")]
    pub DbName: String,
    #[serde(rename = "table_name", skip_serializing_if = "String::is_empty")]
    pub TableName: String,
    #[serde(rename = "partition_name", skip_serializing_if = "String::is_empty")]
    pub PartitionName: String,
    #[serde(rename = "handle_type", skip_serializing_if = "str::is_empty")]
    pub HandleType: HandleType,
    #[serde(rename = "handle_value", skip_serializing_if = "String::is_empty")]
    pub HandleValue: String,
    #[serde(rename = "index_name", skip_serializing_if = "String::is_empty")]
    pub IndexName: String,
    #[serde(rename = "index_values", skip_serializing_if = "Vec::is_empty")]
    pub IndexValues: Vec<String>,
    #[serde(rename = "db_id", skip_serializing_if = "is_zero")]
    pub DbID: i64,
    #[serde(rename = "table_id")]
    pub TableID: i64,
    #[serde(rename = "partition_id", skip_serializing_if = "is_zero")]
    pub PartitionID: i64,
    #[serde(rename = "index_id", skip_serializing_if = "is_zero")]
    pub IndexID: i64,
    #[serde(rename = "partition_handle", skip_serializing_if = "is_false")]
    pub IsPartitionHandle: bool,
}

/// serde 辅助：零值 i64 视为可跳过。
fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// serde 辅助：`false` 视为可跳过。
fn is_false(value: &bool) -> bool {
    !*value
}

/// 键解码失败错误，携带可读消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeKeyError(String);

impl DecodeKeyError {
    /// 由消息构造错误。
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DecodeKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DecodeKeyError {}

/// 根据具体 Handle 实现返回诊断用类型标签；分区 Handle 递归解析内层。
fn handleType(handle: &dyn kv::Handle) -> HandleType {
    if handle.as_any().is::<kv::IntHandle>() {
        IntHandle
    } else if handle.as_any().is::<kv::CommonHandle>() {
        CommonHandle
    } else if let Some(partition) = handle.as_any().downcast_ref::<kv::PartitionHandle>() {
        handleType(partition.Handle.as_ref())
    } else {
        logutil::log::BgLogger().warn(format!(
            "Unexpected kv.Handle type: {}",
            std::any::type_name_of_val(handle)
        ));
        UnknownHandle
    }
}

/// 从 infoschema 解析出的库表/分区/索引元数据快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct KeyMetadata {
    pub(crate) db_id: i64,
    pub(crate) db_name: String,
    pub(crate) table_id: i64,
    pub(crate) table_name: String,
    pub(crate) partition_id: i64,
    pub(crate) partition_name: String,
    pub(crate) indices: Vec<(i64, String)>,
    pub(crate) table_found: bool,
    /// 表存在但无法解析所属 schema（库）时为 true，解码将提前返回部分身份信息。
    pub(crate) schema_missing_for_table: bool,
}

/// 按物理表或分区 ID 查询元数据的抽象，便于单测注入假实现。
pub(crate) trait MetadataLookup {
    fn lookup(&self, table_or_partition_id: i64) -> KeyMetadata;
}

/// 基于 infoschema 的元数据查找适配器。
struct InfoSchemaLookup<'a>(&'a dyn infoschema::InfoSchema);

impl MetadataLookup for InfoSchemaLookup<'_> {
    fn lookup(&self, table_or_partition_id: i64) -> KeyMetadata {
        // 先按表 ID 查找；找不到再按分区 ID 反查所属表。
        if let Some(table) = self.0.TableByID(table_or_partition_id) {
            let meta = table.Meta();
            // Go's SchemaByTable resolves the owning schema from the table's
            // metadata, so use the stable schema ID rather than round-tripping
            // through a display-name lookup.
            let Some(schema) = self.0.SchemaByID(meta.db_id) else {
                logutil::log::BgLogger().warn(format!(
                    "no schema associated with table found in infoschema: {table_or_partition_id}"
                ));
                return KeyMetadata {
                    table_id: table_or_partition_id,
                    table_name: meta.name.original.clone(),
                    table_found: true,
                    schema_missing_for_table: true,
                    ..KeyMetadata::default()
                };
            };
            return KeyMetadata {
                db_id: schema.id,
                db_name: schema.name.original.clone(),
                table_id: meta.id,
                table_name: meta.name.original.clone(),
                indices: meta
                    .indices
                    .iter()
                    .map(|index| (index.id, index.name.original.clone()))
                    .collect(),
                table_found: true,
                ..KeyMetadata::default()
            };
        }

        // 分区物理 ID：还原逻辑表、库与分区名。
        let partition_lookup = self.0.FindTableByPartitionID(table_or_partition_id);
        let table_found = partition_lookup.is_some();
        let mut metadata = KeyMetadata {
            table_id: table_or_partition_id,
            table_found,
            ..KeyMetadata::default()
        };
        if let Some((table, schema, partition)) = partition_lookup {
            metadata.table_id = table.Meta().id;
            metadata.table_name = table.Meta().name.original.clone();
            metadata.indices = table
                .Meta()
                .indices
                .iter()
                .map(|index| (index.id, index.name.original.clone()))
                .collect();
            metadata.db_id = schema.id;
            metadata.db_name = schema.name.original.clone();
            metadata.partition_id = partition.id;
            metadata.partition_name = partition.name.original;
        }
        if !metadata.table_found {
            logutil::log::BgLogger().warn(format!(
                "no table found in infoschema: {table_or_partition_id}"
            ));
        }
        metadata
    }
}

/// 用可注入的 MetadataLookup 解码行键或索引键，供诊断与单测共用。
pub(crate) fn decodeKeyWithLookup(
    key: &[u8],
    lookup: &dyn MetadataLookup,
) -> Result<DecodedKey, DecodeKeyError> {
    let mut result = DecodedKey::default();
    if !tablecodec::IsRecordKey(key) && !tablecodec::IsIndexKey(key) {
        return Err(DecodeKeyError::new(format!(
            "Unknown key type for key {key:?}"
        )));
    }

    // 解出键头：物理表/分区 ID、索引 ID，以及是否为行记录键。
    let key = kv::Key(key.to_vec());
    let (table_or_partition_id, index_id, is_record_key) =
        tablecodec::DecodeKeyHead(key.clone())
            .map_err(|error| DecodeKeyError::new(error.to_string()))?;
    result.TableID = table_or_partition_id;

    let metadata = lookup.lookup(table_or_partition_id);
    if metadata.table_found {
        // 分区键场景下 TableID 改为逻辑表 ID。
        result.TableID = metadata.table_id;
    }
    result.TableName = metadata.table_name;
    result.DbID = metadata.db_id;
    result.DbName = metadata.db_name;
    result.PartitionID = metadata.partition_id;
    result.PartitionName = metadata.partition_name;

    // Go returns the partially resolved identity immediately when a table was
    // found but its owning schema cannot be recovered.
    // 表已找到但所属 schema 缺失时，立刻返回部分身份（与 Go 一致）。
    if metadata.schema_missing_for_table {
        return Ok(result);
    }

    if is_record_key {
        let (_, handle) = tablecodec::DecodeRecordKey(key).map_err(|error| {
            logutil::log::BgLogger().warn(format!(
                "decode record key failed for table {table_or_partition_id}: {error}"
            ));
            DecodeKeyError::new(format!(
                "cannot decode record key of table {table_or_partition_id}"
            ))
        })?;
        result.HandleType = handleType(handle.as_ref());
        result.IsPartitionHandle = handle.as_any().is::<kv::PartitionHandle>();
        result.HandleValue = handle.String();
    } else {
        let (_, _, index_values) = match tablecodec::DecodeIndexKey(key) {
            Ok(decoded) => decoded,
            Err(error) => {
                // Diagnostic callers prefer a partial table identity over an
                // error when only the index payload is stale or malformed.
                // 索引载荷损坏时仍返回已解析的表身份，便于诊断。
                logutil::log::BgLogger().warn(format!("cannot decode index key: {error}"));
                return Ok(result);
            }
        };
        result.IndexID = index_id;
        if metadata.table_found {
            // 用元数据中的索引 ID 映射索引名，并填入解码出的索引列值。
            if let Some((_, name)) = metadata
                .indices
                .iter()
                .find(|(candidate, _)| *candidate == index_id)
            {
                result.IndexName = name.clone();
                result.IndexValues = index_values;
            }
        }
    }
    Ok(result)
}

/// Decodes a TiDB record or index key for diagnostic JSON output.
/// 解码 TiDB 行键或索引键，结合 infoschema 填充诊断用 JSON 字段。
pub fn DecodeKey(
    key: impl AsRef<[u8]>,
    info_schema: &dyn infoschema::InfoSchema,
) -> Result<DecodedKey, DecodeKeyError> {
    decodeKeyWithLookup(key.as_ref(), &InfoSchemaLookup(info_schema))
}
