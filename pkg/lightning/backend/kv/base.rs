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
// Copyright 2026 AsterSQL.

// Lightning SQL→KV 编码的基础实现。
//
// 提供表定义、生成列表达式、`BaseKVEncoder`（将一行 Datum 写入会话事务中的记录键与索引键），
// 以及日志截断、自增/自随机 rebase、生成列求值等辅助逻辑。Handle（行句柄）由主键或隐式
// RowID 决定，记录键形如 `t{tableID}_r{handle}`，索引键形如 `t{tid}_i{iid}_{cols}_h{handle}`。

use std::any::Any;
use std::collections::HashMap;

use encode::{Column, Datum, EncodingConfig, Table};

use crate::{
    AllocatorType, Allocators, GetAutoRecordID, NewPanickingAllocators, NewSession, Pairs, Session,
    canonicalHandle, canonicalIndexInfo, canonicalTableInfo, encodeCanonicalRow, toCanonicalDatum,
};

/// 单行日志序列化时的最大累计长度，超出则截断并附加 truncated 标记。
const maxLogLength: usize = 512 * 1024;

/// 生成列表达式的简化 AST：拷贝列、两列相加或常量。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GeneratedExpression {
    Copy(usize),
    Add(usize, usize),
    Constant(i64),
}

/// 生成列：目标列下标及其表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedCol {
    pub Index: usize,
    pub Expr: GeneratedExpression,
}

/// 自增 ID 字段的数值类型（整型或浮点），供 `GetAutoRecordID` 解析。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoIDFieldType {
    Integer,
    Float,
}

/// 索引定义：ID、构成列下标、是否主键索引。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexDefinition {
    pub id: i64,
    pub columns: Vec<usize>,
    pub primary: bool,
    pub unique: bool,
}

/// 编码用表元数据：列、默认值、生成列、索引、handle 形态与分配器。
///
/// - `pk_is_handle`：主键即 handle（常见整型主键）；
/// - `common_handle`：联合主键作为 Common Handle；
/// - `shard_row_id_bits` / `auto_random_bits`：RowID / AUTO_RANDOM 分片位数，用于打散热点。
#[derive(Clone)]
pub struct TableDefinition {
    pub name: String,
    pub id: i64,
    pub columns: Vec<Column>,
    pub defaults: HashMap<usize, Datum>,
    pub generated: HashMap<usize, GeneratedExpression>,
    pub indices: Vec<IndexDefinition>,
    pub pk_is_handle: bool,
    pub common_handle: bool,
    pub shard_row_id_bits: u8,
    pub auto_random_bits: u8,
    pub allocators: Allocators,
}

impl Default for TableDefinition {
    fn default() -> Self {
        Self {
            name: "table".into(),
            id: 1,
            columns: Vec::new(),
            defaults: HashMap::new(),
            generated: HashMap::new(),
            indices: Vec::new(),
            pk_is_handle: false,
            common_handle: false,
            shard_row_id_bits: 0,
            auto_random_bits: 0,
            allocators: NewPanickingAllocators(false),
        }
    }
}

impl Table for TableDefinition {
    fn name(&self) -> &str {
        &self.name
    }
    fn columns(&self) -> &[Column] {
        &self.columns
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 将原始自增 ID 映射为带分片位的 RowID / AUTO_RANDOM 值的函数类型。
pub type AutoIDConverterFn = fn(i64) -> i64;

/// 将一行 Datum 序列化为日志字段对，并在超长时截断。
pub struct RowArrayMarshaller<'a>(pub &'a [Datum]);

impl RowArrayMarshaller<'_> {
    /// 产出 (类型名, 值字符串) 列表，累计长度超过 `maxLogLength` 时停止并标记 truncated。
    pub fn MarshalLogArray(&self) -> Vec<(String, String)> {
        let mut total = 0;
        let mut result = Vec::new();
        for datum in self.0 {
            let (kind, mut value) = datumKindAndValue(datum);
            // 单值过长时先截断到 1024 并标注。
            if value.len() > maxLogLength {
                value.truncate(1024);
                value.push_str(" (truncated)");
            }
            total += value.len();
            if total >= maxLogLength {
                result.push(("truncated".into(), "The row has been truncated".into()));
                break;
            }
            result.push((kind.into(), value));
        }
        result
    }
}

/// 将 Datum 转为日志用的类型标签与字符串值。
fn datumKindAndValue(datum: &Datum) -> (&'static str, String) {
    match datum {
        Datum::Null => ("null", "NULL".into()),
        Datum::MinNotNull => ("min", "-inf".into()),
        Datum::MaxValue => ("max", "+inf".into()),
        Datum::Int(value) => ("int64", value.to_string()),
        Datum::UInt(value) => ("uint64", value.to_string()),
        Datum::Float(value) => ("float64", value.to_string()),
        Datum::Bytes(value) => ("bytes", String::from_utf8_lossy(value).into_owned()),
        Datum::String(value) => ("string", value.clone()),
        Datum::Json(value) => ("json", value.clone()),
        Datum::BinaryLiteral(value) => ("binary", String::from_utf8_lossy(value).into_owned()),
        Datum::Bit(value) => ("bit", String::from_utf8_lossy(value).into_owned()),
        Datum::Enum { name, value } => ("enum", format!("{name}:{value}")),
        Datum::Set { name, value } => ("set", format!("{name}:{value}")),
        Datum::Decimal(value) => ("decimal", value.clone()),
        Datum::Timestamp(value) => ("time", value.clone()),
        Datum::Duration(value) => ("duration", value.clone()),
    }
}

/// 基础 KV 编码器：持有会话、表元数据、生成列与 AUTO_ID 转换函数。
pub struct BaseKVEncoder {
    pub GenCols: Vec<GeneratedCol>,
    pub SessionCtx: Session,
    pub table: TableDefinition,
    pub Columns: Vec<Column>,
    /// AUTO_RANDOM 列的 1-based 列 ID；无则为 0。
    pub AutoRandomColID: i64,
    pub AutoIDFn: Box<dyn Fn(i64) -> i64 + Send>,
    /// 复用的行缓冲，避免频繁分配。
    recordCache: Vec<Datum>,
}

/// 按 `EncodingConfig` 构造 `BaseKVEncoder`，配置 AUTO_ID 分片混洗函数。
pub fn NewBaseKVEncoder(config: &EncodingConfig) -> Result<BaseKVEncoder, String> {
    let table = config
        .Table
        .as_ref()
        .and_then(|table| table.as_any().downcast_ref::<TableDefinition>())
        .cloned()
        .ok_or_else(|| "encoding table must be a TableDefinition".to_string())?;
    let session = NewSession(&config.SessionOptions)?;
    let genCols = CollectGeneratedColumnsFromTable(&table);
    let seed = config.SessionOptions.AutoRandomSeed as u64;
    let shard_bits = table.shard_row_id_bits.max(table.auto_random_bits);
    let identity = config.UseIdentityAutoRowID || shard_bits == 0;
    // identity：直接返回 rowID；否则用种子混洗高位分片，降低 Region 热点。
    let autoIDFn: Box<dyn Fn(i64) -> i64 + Send> = if identity {
        Box::new(|id| id)
    } else {
        let mask = (1_u64 << shard_bits.min(30)) - 1;
        Box::new(move |id| {
            let mixed = (id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ seed;
            let shift = 63 - shard_bits.min(30);
            (((mixed & mask) << shift) | (id as u64 & ((1_u64 << shift) - 1))) as i64
        })
    };
    let autoRandomColID = table
        .columns
        .iter()
        .position(|column| column.auto_random)
        .map_or(0, |index| index as i64 + 1);
    Ok(BaseKVEncoder {
        GenCols: genCols,
        SessionCtx: session,
        Columns: table.columns.clone(),
        table,
        AutoRandomColID: autoRandomColID,
        AutoIDFn: autoIDFn,
        recordCache: Vec::new(),
    })
}

impl BaseKVEncoder {
    /// 取出并清空可复用的行缓冲。
    pub fn GetOrCreateRecord(&mut self) -> Vec<Datum> {
        std::mem::take(&mut self.recordCache)
    }

    /// 将记录写入会话事务并取出 KV 对；失败时用原始行生成带上下文的错误信息。
    pub fn Record2KV(
        &mut self,
        record: Vec<Datum>,
        originalRow: &[Datum],
        rowID: i64,
    ) -> Result<Pairs, String> {
        if let Err(error) = self.AddRecord(&record, rowID) {
            return Err(self.LogKVConvertFailed(originalRow, -1, "record", &error));
        }
        let mut pairs = self.SessionCtx.TakeKvPairs();
        pairs.RowID = encodeComparableVarint(rowID);
        self.recordCache = record;
        self.recordCache.clear();
        Ok(pairs)
    }

    /// 向当前事务写入记录键与所有二级索引键，返回实际使用的 handle。
    pub fn AddRecord(&mut self, record: &[Datum], rowID: i64) -> Result<i64, String> {
        let handle = canonicalHandle(&self.table, record, rowID)?;
        let record_key = tablecodec::EncodeRowKeyWithHandle(self.table.id, handle.Copy()).0;
        let record_value = encodeCanonicalRow(
            record,
            &self.table.columns,
            self.SessionCtx.GetTableCtx().RowEncodingEnabled,
        )?;
        self.SessionCtx.Txn().Set(&record_key, &record_value)?;
        let table_info = canonicalTableInfo(
            &self.table.columns,
            &self.table.indices,
            self.table.pk_is_handle,
            self.table.common_handle,
        );
        for index in &self.table.indices {
            let values = index
                .columns
                .iter()
                .filter_map(|column| record.get(*column).cloned())
                .collect::<Vec<_>>();
            let canonical_values = values
                .iter()
                .map(toCanonicalDatum)
                .collect::<Result<Vec<_>, _>>()?;
            let index_info = canonicalIndexInfo(index);
            let (key, distinct) = tablecodec::GenIndexKey(
                tablecodec::codec::NewEncoder(false),
                Some(tablecodec::time::UTC),
                Box::new(table_info.clone()),
                Box::new(index_info.clone()),
                self.table.id,
                canonical_values.clone(),
                Some(handle.Copy()),
                None,
            )
            .map_err(|error| error.to_string())?;
            let value = tablecodec::GenIndexValuePortal(
                false,
                Some(tablecodec::time::UTC),
                Box::new(table_info.clone()),
                Box::new(index_info),
                false,
                distinct,
                false,
                canonical_values,
                handle.Copy(),
                0,
                Vec::new(),
                None,
            )
            .map_err(|error| error.to_string())?;
            self.SessionCtx.Txn().Set(&key, &value)?;
        }
        Ok(if handle.IsInt() {
            handle.IntValue()
        } else {
            rowID
        })
    }

    pub fn TableAllocators(&self) -> Allocators {
        self.table.allocators.clone()
    }
    pub fn TableMeta(&self) -> &TableDefinition {
        &self.table
    }

    /// 处理单列 Datum：取实际值后，对 AUTO_RANDOM / AUTO_INCREMENT 列 rebase 分配器。
    pub fn ProcessColDatum(
        &self,
        colIndex: usize,
        rowID: i64,
        inputDatum: Option<&Datum>,
        _needCast: bool,
    ) -> Result<Datum, String> {
        let value = self.getActualDatum(colIndex, rowID, inputDatum, true)?;
        let column = &self.Columns[colIndex];
        if column.auto_random {
            let base = match value {
                Datum::Int(value) => value,
                Datum::UInt(value) => value as i64,
                _ => return Err("auto-random column requires integer".into()),
            };
            self.TableAllocators()
                .Get(AllocatorType::AutoRandomType)
                .Rebase(base, false);
        }
        if column.auto_increment {
            self.TableAllocators()
                .Get(AllocatorType::AutoIncrementType)
                .Rebase(GetAutoRecordID(&value, AutoIDFieldType::Integer), false);
        }
        Ok(value)
    }

    /// 解析列的实际写入值：非空输入优先，否则自增/自随机用 AutoIDFn(rowID)，再否则用默认值。
    pub fn getActualDatum(
        &self,
        colIndex: usize,
        rowID: i64,
        inputDatum: Option<&Datum>,
        _needCast: bool,
    ) -> Result<Datum, String> {
        let column = self
            .Columns
            .get(colIndex)
            .ok_or_else(|| "column out of range".to_string())?;
        if let Some(value) = inputDatum.filter(|value| !matches!(value, Datum::Null)) {
            return Ok(value.clone());
        }
        if column.auto_increment || column.auto_random {
            return Ok(Datum::Int((self.AutoIDFn)(rowID)));
        }
        Ok(self
            .table
            .defaults
            .get(&colIndex)
            .cloned()
            .unwrap_or(Datum::Null))
    }

    pub fn IsAutoRandomCol(&self, colIndex: usize) -> bool {
        self.Columns
            .get(colIndex)
            .is_some_and(|column| column.auto_random)
    }

    pub fn EvalGeneratedColumns(&self, record: &mut [Datum]) -> Result<(), (usize, String)> {
        evalGeneratedColumns(record, &self.GenCols)
    }

    /// 构造 KV 转换失败的诊断日志（含列信息与截断后的行内容）。
    pub fn LogKVConvertFailed(&self, row: &[Datum], j: i32, colInfo: &str, err: &str) -> String {
        format!(
            "kv convert failed at column {colInfo}({j}): {err}; row={:?}",
            RowArrayMarshaller(row).MarshalLogArray()
        )
    }

    /// 构造生成列表达式求值失败的诊断日志。
    pub fn LogEvalGenExprFailed(&self, row: &[Datum], colInfo: &str, err: &str) -> String {
        format!(
            "generated expression failed at {colInfo}: {err}; row={:?}",
            RowArrayMarshaller(row).MarshalLogArray()
        )
    }

    pub fn TruncateWarns(&self) {}
}

/// 将 Datum 转为类型转换错误场景下的可读字符串。
pub fn datumToValueStringForCastError(datum: &Datum) -> String {
    match datum {
        Datum::String(value) => format!("{value:?}"),
        Datum::Bytes(value) => match std::str::from_utf8(value) {
            Ok(value) if value.chars().all(|character| !character.is_control()) => {
                format!("{value:?}")
            }
            _ => format!(
                "0x{}",
                value
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            ),
        },
        _ => datumKindAndValue(datum).1,
    }
}

/// Encode an integer with TiDB's order-preserving varint format.
fn encodeComparableVarint(value: i64) -> Vec<u8> {
    const NEGATIVE_TAG_END: u8 = 8;
    const POSITIVE_TAG_START: u8 = 0xff - 8;
    if value < 0 {
        let bytes = value.to_be_bytes();
        let payload_len = (1..=7)
            .find(|len| value >= -(1_i64 << (len * 8)).saturating_sub(1))
            .unwrap_or(8);
        let mut encoded = Vec::with_capacity(payload_len + 1);
        encoded.push(NEGATIVE_TAG_END - payload_len as u8);
        encoded.extend_from_slice(&bytes[8 - payload_len..]);
        return encoded;
    }
    let value = value as u64;
    if value <= (POSITIVE_TAG_START - NEGATIVE_TAG_END) as u64 {
        return vec![value as u8 + NEGATIVE_TAG_END];
    }
    let bytes = value.to_be_bytes();
    let payload_len = ((64 - value.leading_zeros() + 7) / 8) as usize;
    let mut encoded = Vec::with_capacity(payload_len + 1);
    encoded.push(POSITIVE_TAG_START + payload_len as u8);
    encoded.extend_from_slice(&bytes[8 - payload_len..]);
    encoded
}

/// 按生成列列表顺序求值并写回 `record`；失败返回 (列下标, 错误信息)。
pub fn evalGeneratedColumns(
    record: &mut [Datum],
    generated: &[GeneratedCol],
) -> Result<(), (usize, String)> {
    for column in generated {
        let value = match column.Expr {
            GeneratedExpression::Copy(source) => record.get(source).cloned(),
            GeneratedExpression::Add(left, right) => match (record.get(left), record.get(right)) {
                (Some(Datum::Int(left)), Some(Datum::Int(right))) => Some(Datum::Int(left + right)),
                _ => None,
            },
            GeneratedExpression::Constant(value) => Some(Datum::Int(value)),
        }
        .ok_or_else(|| (column.Index, "cannot evaluate generated expression".into()))?;
        if let Some(target) = record.get_mut(column.Index) {
            *target = value;
        } else {
            return Err((column.Index, "generated column out of range".into()));
        }
    }
    Ok(())
}

/// 从表定义收集生成列并按目标下标排序。
pub(crate) fn CollectGeneratedColumnsFromTable(table: &TableDefinition) -> Vec<GeneratedCol> {
    let mut result = table
        .generated
        .iter()
        .map(|(index, expression)| GeneratedCol {
            Index: *index,
            Expr: expression.clone(),
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|column| column.Index);
    result
}
