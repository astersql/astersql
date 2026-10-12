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

// Hash Join v2 行表元数据（JoinTableMeta）：描述编码行布局、join key 模式与 used 标志。
//
// 构建侧把行编码进连续内存时，需要知道 key 是单 int64、定长序列化还是变长序列化，
// null map 长度，以及列在 row_data 中的保存顺序；探测侧据此切 key、读 null、置 used。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// 字段类型粗分类，用于决定 key 是否定长、是否需要序列化。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    SignedInt,
    UnsignedInt,
    Float,
    Decimal,
    Bytes,
    Text { collation: String },
    DateTime,
    Json,
}
/// 列的逻辑类型：种类、可选定长，以及是否可空。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldType {
    pub kind: FieldKind,
    pub fixed_length: Option<usize>,
    pub nullable: bool,
}
/// Join key 在编码行中的存放模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMode {
    /// 单一 8 字节整数 key，可走最快比较路径。
    OneInt64,
    /// 多个/非内联定长 key，按固定字节序列比较。
    FixedSerialized,
    /// 含变长列的 key，长度随行变化。
    VariableSerialized,
}
/// 单个 key 列的属性：定长字节数（若有）以及是否必须序列化。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyProperty {
    pub fixed_length: Option<usize>,
    pub requires_serialization: bool,
    pub can_be_inlined: bool,
    pub is_integer: bool,
    pub is_unsigned: bool,
}

/// 编码后的构建行：字节载荷、null map、key/row_data 偏移，以及并发 used 标志。
#[derive(Debug)]
pub struct EncodedRow {
    pub bytes: Vec<u8>,
    pub null_map: Vec<u8>,
    pub key_offset: usize,
    pub key_length: usize,
    pub row_data_offset: usize,
    pub used: AtomicBool,
}
impl Clone for EncodedRow {
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            null_map: self.null_map.clone(),
            key_offset: self.key_offset,
            key_length: self.key_length,
            row_data_offset: self.row_data_offset,
            // clone 时复制当前 used 快照到新的原子变量，两侧此后互不影响。
            used: AtomicBool::new(self.used.load(Ordering::Relaxed)),
        }
    }
}

/// 行表布局描述：key 模式、列顺序、null map 长度，以及是否需要 used 标志。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinTableMeta {
    pub build_key_indices: Vec<usize>,
    pub build_types: Vec<FieldType>,
    pub key_mode: KeyMode,
    pub fixed_key_length: usize,
    pub row_columns_order: Vec<usize>,
    pub column_sizes: Vec<Option<usize>>,
    pub null_map_length: usize,
    pub need_used_flag: bool,
    pub saved_column_count: usize,
}
impl JoinTableMeta {
    /// 返回该编码行的序列化 key 长度。
    pub fn serialized_key_length(&self, row: &EncodedRow) -> usize {
        row.key_length
    }
    /// 切出编码行中的 key 字节切片。
    pub fn key_bytes<'a>(&self, row: &'a EncodedRow) -> &'a [u8] {
        &row.bytes[row.key_offset..row.key_offset + row.key_length]
    }
    /// 按列下标读取 null map 中对应 bit。
    pub fn is_column_null(&self, row: &EncodedRow, column_index: usize) -> bool {
        let byte = column_index / 8;
        let bit = column_index % 8;
        row.null_map
            .get(byte)
            .is_some_and(|value| value & (1 << bit) != 0)
    }
    /// 返回 row_data 起始偏移，供探测侧追加构建列时定位。
    pub fn advance_to_row_data(&self, row: &EncodedRow) -> usize {
        row.row_data_offset
    }
    /// 在需要 used 标志的连接类型中，原子置位“该构建行已被探测命中”。
    pub fn set_used_flag(&self, row: &EncodedRow) {
        if self.need_used_flag {
            row.used.store(true, Ordering::Relaxed);
        }
    }
    /// 非原子读取 used：无需标志时视为已使用；否则读 Relaxed。
    pub fn is_current_row_used(&self, row: &EncodedRow) -> bool {
        !self.need_used_flag || row.used.load(Ordering::Relaxed)
    }
    /// 原子读取 used（Acquire），供并发探测路径判断构建行是否已被命中。
    pub fn is_current_row_used_atomic(&self, row: &EncodedRow) -> bool {
        !self.need_used_flag || row.used.load(Ordering::Acquire)
    }
}

/// 根据字段类型推断 key 定长与是否需要序列化。
pub fn key_property(field_type: &FieldType) -> KeyProperty {
    match field_type.kind {
        FieldKind::SignedInt => KeyProperty {
            fixed_length: field_type.fixed_length.or(Some(8)),
            requires_serialization: false,
            can_be_inlined: true,
            is_integer: true,
            is_unsigned: false,
        },
        FieldKind::UnsignedInt => KeyProperty {
            fixed_length: field_type.fixed_length.or(Some(8)),
            requires_serialization: false,
            can_be_inlined: true,
            is_integer: true,
            is_unsigned: true,
        },
        FieldKind::Float => KeyProperty {
            fixed_length: field_type.fixed_length.or(Some(8)),
            requires_serialization: true,
            can_be_inlined: false,
            is_integer: false,
            is_unsigned: false,
        },
        FieldKind::DateTime => KeyProperty {
            fixed_length: Some(8),
            requires_serialization: true,
            can_be_inlined: false,
            is_integer: true,
            is_unsigned: true,
        },
        FieldKind::Decimal => KeyProperty {
            fixed_length: None,
            requires_serialization: true,
            can_be_inlined: false,
            is_integer: false,
            is_unsigned: false,
        },
        FieldKind::Bytes => KeyProperty {
            fixed_length: None,
            requires_serialization: false,
            can_be_inlined: true,
            is_integer: false,
            is_unsigned: false,
        },
        FieldKind::Text { .. } | FieldKind::Json => KeyProperty {
            fixed_length: None,
            requires_serialization: true,
            can_be_inlined: false,
            is_integer: false,
            is_unsigned: false,
        },
    }
}

/// 根据构建/探测 key、other condition 列与输出列计算 `JoinTableMeta`。
///
/// 选择 KeyMode：单一兼容 int64 → OneInt64；全部定长 → FixedSerialized；否则 VariableSerialized。
/// 列顺序优先 key、再 other condition、再输出，最后补齐未列出的构建列。
pub fn new_table_meta(
    build_key_indices: &[usize],
    build_types: &[FieldType],
    build_key_types: &[FieldType],
    probe_key_types: &[FieldType],
    columns_used_by_other_condition: &[usize],
    output_columns: &[usize],
    need_used_flag: bool,
) -> Result<JoinTableMeta, String> {
    if build_key_indices.len() != build_key_types.len()
        || build_key_types.len() != probe_key_types.len()
    {
        return Err("join key metadata length mismatch".into());
    }
    if build_key_indices
        .iter()
        .any(|index| *index >= build_types.len())
    {
        return Err("build key index is out of range".into());
    }
    let properties: Vec<_> = build_key_types.iter().map(key_property).collect();
    let probe_properties: Vec<_> = probe_key_types.iter().map(key_property).collect();
    if properties
        .iter()
        .zip(&probe_properties)
        .any(|(build, probe)| build.is_integer && !probe.is_integer)
    {
        return Err("build key is integer but probe key is not integer".into());
    }
    let needs_sign_flag: Vec<_> = properties
        .iter()
        .zip(&probe_properties)
        .map(|(build, probe)| build.is_integer && build.is_unsigned != probe.is_unsigned)
        .collect();
    let compatible_one_int =
        properties.len() == 1 && properties[0].is_integer && !needs_sign_flag[0];
    let fixed_key_length = properties
        .iter()
        .map(|property| property.fixed_length.unwrap_or(0))
        .sum::<usize>()
        + needs_sign_flag.iter().filter(|needed| **needed).count();
    let key_mode = if compatible_one_int {
        KeyMode::OneInt64
    } else if properties
        .iter()
        .all(|property| property.fixed_length.is_some())
    {
        KeyMode::FixedSerialized
    } else {
        KeyMode::VariableSerialized
    };
    let keys_are_unique =
        build_key_indices.iter().collect::<BTreeSet<_>>().len() == build_key_indices.len();
    let keys_are_inlined = keys_are_unique
        && properties.iter().all(|property| property.can_be_inlined)
        && !needs_sign_flag.iter().any(|needed| *needed);
    // Go 的非 nil 空 outputColumns 表示不保存输出列；只有内联 key 仍须进 row data。
    let mut seen = BTreeSet::new();
    let mut row_columns_order = Vec::new();
    let ordered_columns = build_key_indices
        .iter()
        .copied()
        .filter(|_| keys_are_inlined)
        .chain(columns_used_by_other_condition.iter().copied())
        .chain(output_columns.iter().copied());
    for index in ordered_columns {
        if index >= build_types.len() {
            return Err("saved column index is out of range".into());
        }
        if seen.insert(index) {
            row_columns_order.push(index);
        }
    }
    let column_sizes = row_columns_order
        .iter()
        .map(|index| build_types[*index].fixed_length)
        .collect();
    let null_map_length = if need_used_flag {
        (seen.len() + 1).div_ceil(32) * 4
    } else {
        seen.len().div_ceil(8)
    };
    Ok(JoinTableMeta {
        build_key_indices: build_key_indices.to_vec(),
        build_types: build_types.to_vec(),
        key_mode,
        fixed_key_length,
        row_columns_order,
        column_sizes,
        null_map_length,
        need_used_flag,
        saved_column_count: seen.len(),
    })
}
