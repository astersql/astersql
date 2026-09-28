// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 二级索引键值生成与 DDL 状态相关辅助（对应 Go `tables/index.go`）。
//
// 负责根据索引列 Datum（单元格值）与行句柄（handle，即主键/行标识）
// 编码索引 Key/Value；在索引 DDL（如增删索引）的 SchemaState 中间态
// 生成临时索引键；并判断索引是否可写、去重索引列定义。

use crate::mutation_checker::Datum;
use std::collections::HashSet;

/// 索引 ID 掩码：清除最高位（临时索引等标记位）后得到真实 index id。
pub const INDEX_ID_MASK: i64 = 0x0000_ffff_ffff_ffff;
/// 临时索引键前缀字节 `'t'`，用于 DDL 重组期间的双写路径。
pub const TEMP_INDEX_PREFIX: u8 = b't';
/// 非临时索引值。
pub const TEMP_INDEX_KEY_TYPE_NONE: u8 = 0;
/// DeleteOnly backfill 删除标记。
pub const TEMP_INDEX_KEY_TYPE_DELETE: u8 = 1;
/// Running backfill 写入标记。
pub const TEMP_INDEX_KEY_TYPE_BACKFILL: u8 = 2;
/// ReadyToMerge/Merging 双写标记。
pub const TEMP_INDEX_KEY_TYPE_MERGE: u8 = 3;

/// Schema 对象状态（对应 TiDB DDL SchemaState）。
///
/// 索引从创建到对用户可见会经历 DeleteOnly → WriteOnly → WriteReorganization → Public。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaState {
    None,
    DeleteOnly,
    WriteOnly,
    WriteReorganization,
    DeleteReorganization,
    Public,
}

/// 在线索引回填状态，决定临时索引键是单写还是与正式索引双写。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackfillState {
    #[default]
    Inapplicable,
    Running,
    ReadyToMerge,
    Merging,
}

/// 列元信息：用于判断前缀索引或新排序规则下是否需要还原数据（restored data）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
    /// 列是否需要在索引 value 中保存原始字节以便还原比较。
    pub needs_restored_data: bool,
}

/// 索引列定义：列名、在表中的偏移、可选前缀长度。
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct IndexColumn {
    pub name: String,
    pub offset: usize,
    /// 前缀索引长度；`Some` 时表示只索引列值前缀。
    pub length: Option<usize>,
}

/// 索引元信息：唯一性、主键标记、DDL 状态及部分索引条件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    pub primary: bool,
    pub state: SchemaState,
    pub backfill_state: BackfillState,
    /// 部分索引（partial index）条件表达式文本；`None` 表示全表索引。
    pub condition: Option<String>,
}

/// 表元信息子集：物理表 ID 与列列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub columns: Vec<ColumnInfo>,
}

/// 判断是否需要在索引 value 中写入 restored data。
///
/// 新排序规则（new collation）下，前缀索引或标记为 needs_restored_data 的列需要保留原始字节。
pub fn need_restored_data(
    use_new_collation: bool,
    index_columns: &[IndexColumn],
    columns: &[ColumnInfo],
) -> bool {
    use_new_collation
        && index_columns.iter().any(|column| {
            column.length.is_some()
                || columns
                    .get(column.offset)
                    .is_some_and(|info| info.needs_restored_data)
        })
}

/// 可执行索引对象：绑定物理表 ID、表/索引元信息及 restored_data 标志。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Index {
    /// 物理表 ID（分区表则为分区物理 ID）。
    pub physical_id: i64,
    pub table_info: TableInfo,
    pub index_info: IndexInfo,
    pub use_new_collation: bool,
    pub restored_data: bool,
}

impl Index {
    /// 构造索引；校验列偏移合法，并计算是否需要 restored data。
    pub fn new(
        use_new_collation: bool,
        physical_id: i64,
        table_info: TableInfo,
        index_info: IndexInfo,
    ) -> Result<Self, IndexError> {
        for column in &index_info.columns {
            if column.offset >= table_info.columns.len() {
                return Err(IndexError::ColumnOffset(column.offset));
            }
        }
        let restored_data =
            need_restored_data(use_new_collation, &index_info.columns, &table_info.columns);
        Ok(Self {
            physical_id,
            table_info,
            index_info,
            use_new_collation,
            restored_data,
        })
    }

    /// 生成索引键；返回 `(key, distinct)`。
    ///
    /// `distinct` 为 true 表示唯一索引且无 NULL：键中不追加 handle；
    /// 否则把 handle 编入键以保证非唯一/含 NULL 时键仍唯一。
    pub fn gen_index_key(
        &self,
        indexed_values: &[Datum],
        handle: i64,
    ) -> Result<(Vec<u8>, bool), IndexError> {
        if indexed_values.len() != self.index_info.columns.len() {
            return Err(IndexError::ValueCount {
                expected: self.index_info.columns.len(),
                actual: indexed_values.len(),
            });
        }
        // 唯一索引且全部非 NULL 时键本身可区分行，无需追加 handle。
        let distinct = self.index_info.unique
            && indexed_values
                .iter()
                .all(|value| !matches!(value, Datum::Null));
        let mut key = format!("t{}_i{}", self.physical_id, self.index_info.id).into_bytes();
        for value in indexed_values {
            encode_datum(value, &mut key);
        }
        if !distinct {
            key.extend_from_slice(&handle.to_be_bytes());
        }
        Ok((key, distinct))
    }

    /// 生成索引 value：可选 untouched 标记、distinct 时的 handle、以及 restored data。
    pub fn gen_index_value(
        &self,
        distinct: bool,
        untouched: bool,
        handle: i64,
        restored_values: &[Datum],
    ) -> Vec<u8> {
        let mut value = Vec::new();
        value.push(u8::from(untouched));
        if distinct {
            value.extend_from_slice(&handle.to_be_bytes());
        }
        if self.restored_data {
            for datum in restored_values {
                encode_datum(datum, &mut value);
            }
        }
        value
    }

    /// 判断行是否满足部分索引条件；无条件时恒为 true。
    pub fn meet_partial_condition(
        &self,
        row: &[Datum],
        evaluate: impl Fn(&str, &[Datum]) -> Result<Option<bool>, IndexError>,
    ) -> Result<bool, IndexError> {
        match self.index_info.condition.as_deref() {
            None => Ok(true),
            Some(condition) => Ok(evaluate(condition, row)?.unwrap_or(false)),
        }
    }
}

/// 将单个 Datum 以简单类型标签 + 载荷形式追加到字节缓冲。
fn encode_datum(value: &Datum, output: &mut Vec<u8>) {
    match value {
        Datum::Null => output.push(0),
        Datum::Int(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Datum::Uint(value) => {
            output.push(2);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Datum::Bytes(value) => {
            output.push(3);
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }
    }
}

/// 按 SchemaState 与 BackfillState 生成临时索引键。
///
/// Running 只写临时索引；ReadyToMerge/Merging 同时写正式与临时索引；
/// Public 或 Inapplicable 只写正式索引。返回 `(正式键, 临时键可选, 版本)`。
pub fn gen_temp_index_key_by_state(
    index: &IndexInfo,
    index_key: &[u8],
) -> (Vec<u8>, Option<Vec<u8>>, u8) {
    if index.state == SchemaState::Public || index.backfill_state == BackfillState::Inapplicable {
        return (index_key.to_vec(), None, TEMP_INDEX_KEY_TYPE_NONE);
    }
    let mut temporary = Vec::with_capacity(index_key.len() + 1);
    temporary.push(TEMP_INDEX_PREFIX);
    temporary.extend_from_slice(index_key);
    match index.backfill_state {
        BackfillState::Running => {
            let version = if index.state == SchemaState::DeleteOnly {
                TEMP_INDEX_KEY_TYPE_DELETE
            } else {
                TEMP_INDEX_KEY_TYPE_BACKFILL
            };
            (Vec::new(), Some(temporary), version)
        }
        BackfillState::ReadyToMerge | BackfillState::Merging => (
            index_key.to_vec(),
            Some(temporary),
            TEMP_INDEX_KEY_TYPE_MERGE,
        ),
        BackfillState::Inapplicable => (index_key.to_vec(), None, TEMP_INDEX_KEY_TYPE_NONE),
    }
}

/// 除 DeleteOnly / DeleteReorganization 外，索引均可写。
pub fn is_index_writable(index: &IndexInfo) -> bool {
    !matches!(
        index.state,
        SchemaState::DeleteOnly | SchemaState::DeleteReorganization
    )
}

/// 按列偏移去重索引列，保留首次出现顺序。
pub fn dedup_index_columns(columns: &[IndexColumn]) -> Vec<IndexColumn> {
    let mut seen = HashSet::new();
    columns
        .iter()
        .filter(|column| seen.insert(column.offset))
        .cloned()
        .collect()
}

/// 索引操作错误：列偏移非法、值个数不匹配、或条件求值失败。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexError {
    ColumnOffset(usize),
    ValueCount { expected: usize, actual: usize },
    Evaluation(String),
}
