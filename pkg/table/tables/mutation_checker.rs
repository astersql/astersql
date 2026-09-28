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

// 行与索引 Mutation 的数据一致性检查（对应 Go `mutation_checker.go`）。
//
// 在事务缓冲（MemBuffer）提交前，校验：索引 PUT 的 handle 与行一致、
// 索引列值与源行对应列一致。分区表、流水线 DML（pipelined DML）或无效
// staging handle 时快速跳过（与 Go 相同）。

use crate::index::INDEX_ID_MASK;
use std::cmp::Ordering;
use std::collections::HashMap;

/// 简化的单元格值类型，供一致性比较使用。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Datum {
    Null,
    Int(i64),
    Uint(u64),
    Bytes(Vec<u8>),
}

/// Mutation 标志位：PresumeKeyNotExists（假设键不存在）与 Untouched（未改动的临时索引值）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MutationFlags {
    pub presume_key_not_exists: bool,
    /// untouched 临时索引值不会提交，检查时跳过。
    pub untouched: bool,
}

/// 一次键值变更：含键、标志、值、索引 ID、可选 handle 与索引列值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Mutation {
    pub key: Vec<u8>,
    pub flags: MutationFlags,
    pub value: Vec<u8>,
    pub index_id: i64,
    pub handle: Option<i64>,
    pub indexed_values: Vec<Datum>,
    /// 从行 value 中实际解码出的 `(列偏移, 值)`；被省略的列不参与行一致性检查。
    pub row_values: Vec<(usize, Datum)>,
}

/// 索引列布局：索引 ID 与各索引列在行中的偏移。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexLayout {
    pub id: i64,
    pub column_offsets: Vec<usize>,
}

/// 一致性检查失败原因。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsistencyError {
    InconsistentRowValue,
    InconsistentHandle {
        row_handle: i64,
        index_handle: i64,
        index_id: i64,
    },
    InconsistentIndexedValue {
        index_id: i64,
        column_offset: usize,
    },
    MissingIndex(i64),
}

/// 入口：与 Go 相同的快退条件——分区表、流水线 DML、无有效 staging handle 时不检查。
/// The same fast-exit conditions as Go: partitioned tables, pipelined DML and
/// MemBuffers without a valid staging handle cannot be checked here.
pub fn check_data_consistency(
    partitioned: bool,
    pipelined: bool,
    staging_handle: u64,
    row_to_insert: Option<&[Datum]>,
    row_to_remove: Option<&[Datum]>,
    row_insertion: Option<&Mutation>,
    index_mutations: &[Mutation],
    index_layouts: &HashMap<i64, IndexLayout>,
) -> Result<(), ConsistencyError> {
    // 分区表 / pipelined / 无 staging 时跳过，避免误报。
    if partitioned || pipelined || staging_handle == 0 {
        return Ok(());
    }
    if let Some(row) = row_insertion.filter(|mutation| !mutation.key.is_empty()) {
        check_handle_consistency(row, index_mutations, index_layouts)?;
    }
    check_index_keys(row_to_insert, row_to_remove, index_mutations, index_layouts)
}

/// 校验行 PUT 中实际编码的列值；未编码列与删除操作不参与比较。
///
/// 与 Go 的独立 `checkRowInsertionConsistency` 保持相同契约。顶层检查当前同 Go
/// 一样不主动调用该高开销检查，但测试和诊断路径可显式使用。
pub fn check_row_insertion_consistency(
    row_to_insert: Option<&[Datum]>,
    row_insertion: &Mutation,
) -> Result<(), ConsistencyError> {
    let Some(row_to_insert) = row_to_insert else {
        return Ok(());
    };
    for (offset, decoded_value) in &row_insertion.row_values {
        let Some(input_value) = row_to_insert.get(*offset) else {
            return Err(ConsistencyError::InconsistentRowValue);
        };
        if compare_index_and_value(input_value, decoded_value, false) != Ordering::Equal {
            return Err(ConsistencyError::InconsistentRowValue);
        }
    }
    Ok(())
}

/// PUT 索引隐含同 handle 的 PUT 行；删除与 untouched 临时索引值跳过（不会提交）。
/// PUT_index implies a PUT_row with the same handle. Deletes and untouched
/// temporary-index values are skipped because they are never committed.
pub fn check_handle_consistency(
    row_insertion: &Mutation,
    index_mutations: &[Mutation],
    index_layouts: &HashMap<i64, IndexLayout>,
) -> Result<(), ConsistencyError> {
    let Some(row_handle) = row_insertion.handle else {
        return Ok(());
    };
    for mutation in index_mutations {
        // 空 value（删除）不参与提交一致性检查。
        if mutation.value.is_empty() {
            continue;
        }
        // 用 INDEX_ID_MASK 去掉临时索引标记位。
        let index_id = mutation.index_id & INDEX_ID_MASK;
        if !index_layouts.contains_key(&index_id) {
            return Err(ConsistencyError::MissingIndex(index_id));
        }
        // Go 仅跳过临时索引编码中的 untouched 值，并且会先确认索引存在。
        if mutation.index_id != index_id && mutation.flags.untouched {
            continue;
        }
        if let Some(index_handle) = mutation.handle {
            if index_handle != row_handle {
                return Err(ConsistencyError::InconsistentHandle {
                    row_handle,
                    index_handle,
                    index_id,
                });
            }
        }
    }
    Ok(())
}

/// 逐索引比对：删除用 `row_to_remove`，写入用 `row_to_insert`，列值须与布局一致。
pub fn check_index_keys(
    row_to_insert: Option<&[Datum]>,
    row_to_remove: Option<&[Datum]>,
    mutations: &[Mutation],
    layouts: &HashMap<i64, IndexLayout>,
) -> Result<(), ConsistencyError> {
    for mutation in mutations {
        let index_id = mutation.index_id & INDEX_ID_MASK;
        let layout = layouts
            .get(&index_id)
            .ok_or(ConsistencyError::MissingIndex(index_id))?;
        // untouched 是临时索引 value 的状态；普通索引不能借此绕过检查。
        if mutation.index_id != index_id && mutation.flags.untouched {
            continue;
        }
        // 空 value 视为删除，对照待删行；否则对照待插入行。
        let source_row = if mutation.value.is_empty() {
            row_to_remove
        } else {
            row_to_insert
        };
        let Some(source_row) = source_row else {
            continue;
        };
        for (index_position, row_offset) in layout.column_offsets.iter().copied().enumerate() {
            let expected = source_row.get(row_offset).unwrap_or(&Datum::Null);
            let actual = mutation
                .indexed_values
                .get(index_position)
                .unwrap_or(&Datum::Null);
            if compare_index_and_value(expected, actual, false) != Ordering::Equal {
                return Err(ConsistencyError::InconsistentIndexedValue {
                    index_id,
                    column_offset: row_offset,
                });
            }
        }
    }
    Ok(())
}

/// 多值索引（multi-valued index）将标量行值与索引值任一成员比较；普通索引做类型感知标量比较。
/// Multi-valued indexes compare a scalar row value against any member of the
/// indexed value; ordinary indexes use type-aware scalar comparison.
pub fn compare_index_and_value(
    row_value: &Datum,
    index_value: &Datum,
    compare_multi_value_index: bool,
) -> Ordering {
    if compare_multi_value_index {
        // 多值索引：以 0 字节分隔的成员列表中是否包含行值。
        if let Datum::Bytes(values) = index_value {
            let needle = datum_bytes(row_value);
            return if values
                .split(|byte| *byte == 0)
                .any(|member| member == needle.as_slice())
            {
                Ordering::Equal
            } else {
                Ordering::Less
            };
        }
    }
    match (row_value, index_value) {
        (Datum::Null, Datum::Null) => Ordering::Equal,
        (Datum::Null, _) => Ordering::Less,
        (_, Datum::Null) => Ordering::Greater,
        (Datum::Int(left), Datum::Int(right)) => left.cmp(right),
        (Datum::Uint(left), Datum::Uint(right)) => left.cmp(right),
        (Datum::Int(left), Datum::Uint(right)) => {
            if *left < 0 {
                Ordering::Less
            } else {
                (*left as u64).cmp(right)
            }
        }
        (Datum::Uint(left), Datum::Int(right)) => {
            if *right < 0 {
                Ordering::Greater
            } else {
                left.cmp(&(*right as u64))
            }
        }
        (Datum::Bytes(left), Datum::Bytes(right)) => left.cmp(right),
        (left, right) => datum_bytes(left).cmp(&datum_bytes(right)),
    }
}

/// 将 Datum 转为可比较的字节序列（跨类型回退比较用）。
fn datum_bytes(value: &Datum) -> Vec<u8> {
    match value {
        Datum::Null => Vec::new(),
        Datum::Int(value) => value.to_string().into_bytes(),
        Datum::Uint(value) => value.to_string().into_bytes(),
        Datum::Bytes(value) => value.clone(),
    }
}
