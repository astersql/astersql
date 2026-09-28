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

// hash join v2 行表元数据如何决定 row layout、join key 内联/序列化模式、
//
// JoinTableMeta 对应 Go joinTableMeta，是 hash join v2 row table 的布局描述。
// pub struct JoinTableMeta {
// isFixedLength 表示整行是否固定长度。
//     pub is_fixed_length: bool,
//     pub row_length: i32,
//     pub is_join_keys_fixed_length: bool,
//     pub join_keys_length: i32,
// join keys 只有在所有 key 可内联且没有重复 key 时才能内联进 row_data。
//     pub is_join_keys_inlined: bool,
// nullMapLength 包含列 null bit，以及右半/外连接需要的 used flag。
//     pub null_map_length: i32,
// rowColumnsOrder 描述 row layout 中保存列的顺序，可能不同于 build schema 顺序。
//     pub row_columns_order: Vec<i32>,
//     pub columns_size: Vec<i32>,
//     pub serialize_modes: Vec<codec::SerializeMode>,
//     pub column_count_needed_for_other_condition: i32,
//     pub total_column_number: i32,
//     pub col_offset_in_null_map: i32,
//     pub key_mode: KeyMode,
// rowDataOffset 为 -1 表示变长且 key 非内联，需要运行时按 serialized key 长度推进。
//     pub row_data_offset: i32,
//     pub fake_key_byte: Vec<u8>,
// }
//
// impl JoinTableMeta {
// getSerializedKeyLength 对应 Go：读取 next ptr 后 null map 后面的 uint32 key 长度。
//     pub fn get_serialized_key_length(&self, row_start: *const u8) -> u32 {
//         unsafe { *(row_start.add(SIZE_OF_NEXT_PTR + self.null_map_length as usize) as *const u32) }
//     }
//
// isReadNullMapThreadSafe 对应 Go：used flag 可能被其他 goroutine 原子写入时，前 31 列不能普通读取。
//     pub fn is_read_null_map_thread_safe(&self, column_index: i32) -> bool {
//         let may_concurrent_write = self.col_offset_in_null_map == 1 && column_index < 31;
//         !may_concurrent_write
//     }
//
// getKeyBytes 对应 Go 测试辅助方法：按 keyMode 从 rowStart 切出 key 字节。
//     pub fn get_key_bytes(&self, row_start: *const u8) -> &[u8] {
//         unsafe {
//             match self.key_mode {
//                 KeyMode::OneInt64 => hack::GetBytesFromPtr(row_start.add(self.null_map_length as usize + SIZE_OF_NEXT_PTR), serialization::Uint64Len as usize),
//                 KeyMode::FixedSerializedKey => hack::GetBytesFromPtr(row_start.add(self.null_map_length as usize + SIZE_OF_NEXT_PTR), self.join_keys_length as usize),
//                 KeyMode::VariableSerializedKey => hack::GetBytesFromPtr(row_start.add(self.null_map_length as usize + SIZE_OF_NEXT_PTR + SIZE_OF_ELEMENT_SIZE), self.get_serialized_key_length(row_start) as usize),
//             }
//         }
//     }
//
// advanceToRowData 对应 Go：非内联变长 key 需要先读 serialized key length，再移动到 row_data。
//     pub fn advance_to_row_data(&self, matched_row_info: &mut MatchedRowInfo) {
//         if self.row_data_offset == -1 {
//             let row_start = matched_row_info.build_row_start as *const u8;
//             matched_row_info.build_row_offset = SIZE_OF_NEXT_PTR + self.null_map_length as usize + SIZE_OF_ELEMENT_SIZE + self.get_serialized_key_length(row_start) as usize;
//         } else {
//             matched_row_info.build_row_offset = self.row_data_offset as usize;
//         }
//     }
//
// isColumnNull 对应 Go：普通读取 null map 的某一 bit。
//     pub fn is_column_null(&self, row_start: *const u8, column_index: i32) -> bool {
//         let byte_index = (column_index + self.col_offset_in_null_map) / 8;
//         let bit_index = (column_index + self.col_offset_in_null_map) % 8;
//         unsafe {
//             (*(row_start.add(SIZE_OF_NEXT_PTR + byte_index as usize)) & (1_u8 << (7 - bit_index))) != 0
//         }
//     }
//
// isColumnNullThreadSafe 对应 Go：有 used flag 并发写时，用 atomic.LoadUint32 避免读写冲突。
//     pub fn is_column_null_thread_safe(&self, row_start: *const u8, column_index: i32) -> bool {
//         let value = unsafe { std::sync::atomic::AtomicU32::from_ptr(row_start.add(SIZE_OF_NEXT_PTR) as *mut u32).load(std::sync::atomic::Ordering::SeqCst) };
//         unsafe { value & BIT_MASK_IN_UINT32[(column_index + 1) as usize] != 0 }
//     }
//
// setUsedFlag/isCurrentRowUsed 对应 Go：used flag 存在 null map 第一个 bit，写入使用 atomic uint32。
//     pub fn set_used_flag(&self, row_start: *mut u8) {
//         let addr = unsafe { std::sync::atomic::AtomicU32::from_ptr(row_start.add(SIZE_OF_NEXT_PTR) as *mut u32) };
//         let value = addr.load(std::sync::atomic::Ordering::SeqCst) | unsafe { USED_FLAG_MASK };
//         addr.store(value, std::sync::atomic::Ordering::SeqCst);
//     }
//
//     pub fn is_current_row_used(&self, row_start: *const u8) -> bool {
//         let value = unsafe { *(row_start.add(SIZE_OF_NEXT_PTR) as *const u32) };
//         value & unsafe { USED_FLAG_MASK } == unsafe { USED_FLAG_MASK }
//     }
//
//     pub fn is_current_row_used_with_atomic(&self, row_start: *const u8) -> bool {
//         let value = unsafe { std::sync::atomic::AtomicU32::from_ptr(row_start.add(SIZE_OF_NEXT_PTR) as *mut u32).load(std::sync::atomic::Ordering::SeqCst) };
//         value & unsafe { USED_FLAG_MASK } == unsafe { USED_FLAG_MASK }
//     }
// }
//
// KeyProp 对应 Go keyProp，描述单个 join key 是否可内联、长度和整数符号属性。
// pub struct KeyProp {
//     pub can_be_inlined: bool,
//     pub key_length: i32,
//     pub is_key_integer: bool,
//     pub is_key_unsigned: bool,
// }
//
// getKeyProp 对应 Go：按 TiDB/MySQL 类型判断 key 的内联能力和序列化长度。
// pub fn get_key_prop(tp: &types::FieldType) -> KeyProp {
//     match tp.GetType() {
//         mysql::TypeTiny | mysql::TypeShort | mysql::TypeInt24 | mysql::TypeLong | mysql::TypeLonglong | mysql::TypeYear | mysql::TypeDuration => {
//             let mut is_key_unsigned = mysql::HasUnsignedFlag(tp.GetFlag());
//             if tp.GetType() == mysql::TypeYear {
//                 is_key_unsigned = true;
//             } else if tp.GetType() == mysql::TypeDuration {
//                 is_key_unsigned = false;
//             }
//             KeyProp { can_be_inlined: true, key_length: chunk::GetFixedLen(tp), is_key_integer: true, is_key_unsigned }
//         }
//         mysql::TypeVarchar | mysql::TypeVarString | mysql::TypeString | mysql::TypeBlob | mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeLongBlob => {
//             let collator = collate::GetCollator(tp.GetCollate());
//             KeyProp { can_be_inlined: collate::CanUseRawMemAsKey(collator), key_length: chunk::VarElemLen, is_key_integer: false, is_key_unsigned: false }
//         }
//         mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
// 日期时间类型序列化为 uint64 key，不能直接内联为原始内存 key。
//             KeyProp { can_be_inlined: false, key_length: serialization::Uint64Len as i32, is_key_integer: true, is_key_unsigned: true }
//         }
//         mysql::TypeFloat => KeyProp { can_be_inlined: false, key_length: serialization::Float64Len as i32, is_key_integer: false, is_key_unsigned: false },
//         mysql::TypeNewDecimal => KeyProp { can_be_inlined: false, key_length: chunk::VarElemLen, is_key_integer: false, is_key_unsigned: false },
//         mysql::TypeEnum => {
//             if mysql::HasEnumSetAsIntFlag(tp.GetFlag()) {
//                 KeyProp { can_be_inlined: false, key_length: serialization::Uint64Len as i32, is_key_integer: true, is_key_unsigned: true }
//             } else {
//                 KeyProp { can_be_inlined: false, key_length: chunk::VarElemLen, is_key_integer: false, is_key_unsigned: false }
//             }
//         }
//         mysql::TypeBit => KeyProp { can_be_inlined: false, key_length: serialization::Uint64Len as i32, is_key_integer: true, is_key_unsigned: true },
//         _ => KeyProp { can_be_inlined: false, key_length: chunk::GetFixedLen(tp), is_key_integer: false, is_key_unsigned: false },
//     }
// }
//
// newTableMeta 对应 Go：根据 build/probe key、other condition、输出列和 used flag 计算 row layout。
// pub fn new_table_meta(
//     build_key_index: Vec<i32>,
//     build_types: Vec<types::FieldType>,
//     build_key_types: Vec<types::FieldType>,
//     probe_key_types: Vec<types::FieldType>,
//     columns_used_by_other_condition: Vec<i32>,
//     output_columns: Option<Vec<i32>>,
//     need_used_flag: bool,
// ) -> JoinTableMeta {
//     let mut meta = JoinTableMeta {
//         is_fixed_length: true,
//         row_length: 0,
//         is_join_keys_fixed_length: true,
//         join_keys_length: 0,
//         is_join_keys_inlined: true,
//         null_map_length: 0,
//         row_columns_order: Vec::new(),
//         columns_size: Vec::new(),
//         serialize_modes: Vec::new(),
//         column_count_needed_for_other_condition: 0,
//         total_column_number: build_types.len() as i32,
//         col_offset_in_null_map: 0,
//         key_mode: KeyMode::VariableSerializedKey,
//         row_data_offset: -1,
//         fake_key_byte: Vec::new(),
//     };
//     let mut columns_need_to_be_saved = std::collections::HashSet::with_capacity(build_types.len());
//     update_columns_need_to_be_saved(&mut meta, &mut columns_need_to_be_saved, &build_types, output_columns.clone(), &columns_used_by_other_condition);
//     setup_join_keys(&mut meta, &build_key_index, &build_key_types, &probe_key_types);
//     if meta.is_join_keys_inlined {
//         for index in &build_key_index {
//             update_one_saved_column(&mut meta, &mut columns_need_to_be_saved, &build_types, *index);
//         }
//     }
//     if !meta.is_fixed_length {
//         meta.row_length = 0;
//     }
//     let saved_column_num = columns_need_to_be_saved.len() as i32;
//     setup_column_order(&mut meta, &build_key_index, &build_types, &columns_used_by_other_condition, output_columns, saved_column_num);
//     if need_used_flag {
//         meta.col_offset_in_null_map = 1;
// used flag 需要被 probe 阶段并发读写，所以 null map 对齐到 4 字节，匹配 atomic.LoadUint32 的最小访问单位。
//         meta.null_map_length = ((saved_column_num + 1 + 31) / 32) * 4;
//     } else {
//         meta.col_offset_in_null_map = 0;
//         meta.null_map_length = (saved_column_num + 7) / 8;
//     }
//     meta.row_data_offset = -1;
//     if meta.is_join_keys_inlined {
//         meta.row_data_offset = if meta.is_join_keys_fixed_length {
//             SIZE_OF_NEXT_PTR as i32 + meta.null_map_length
//         } else {
//             SIZE_OF_NEXT_PTR as i32 + meta.null_map_length + SIZE_OF_ELEMENT_SIZE as i32
//         };
//     } else if meta.is_join_keys_fixed_length {
//         meta.row_data_offset = SIZE_OF_NEXT_PTR as i32 + meta.null_map_length + meta.join_keys_length;
//     }
//     if meta.is_join_keys_fixed_length && !meta.is_join_keys_inlined {
//         meta.fake_key_byte = vec![0; meta.join_keys_length as usize];
//     }
//     meta
// }
//
// update_columns_need_to_be_saved 对应 Go newTableMeta 内部闭包 updateMeta 的调用顺序。
// fn update_columns_need_to_be_saved(
//     meta: &mut JoinTableMeta,
//     columns_need_to_be_saved: &mut std::collections::HashSet<i32>,
//     build_types: &[types::FieldType],
//     output_columns: Option<Vec<i32>>,
//     columns_used_by_other_condition: &[i32],
// ) {
//     if let Some(output_columns) = output_columns {
//         for index in output_columns {
//             update_one_saved_column(meta, columns_need_to_be_saved, build_types, index);
//         }
//         for index in columns_used_by_other_condition {
//             update_one_saved_column(meta, columns_need_to_be_saved, build_types, *index);
//         }
//     } else {
// outputColumns = nil 表示输出需要 build side 的全部列。
//         for index in 0..build_types.len() as i32 {
//             update_one_saved_column(meta, columns_need_to_be_saved, build_types, index);
//         }
//     }
// }
//
// fn update_one_saved_column(meta: &mut JoinTableMeta, columns_need_to_be_saved: &mut std::collections::HashSet<i32>, build_types: &[types::FieldType], index: i32) {
//     if columns_need_to_be_saved.insert(index) {
//         let length = chunk::GetFixedLen(&build_types[index as usize]);
//         if length == chunk::VarElemLen {
//             meta.is_fixed_length = false;
//         } else {
//             meta.row_length += length;
//         }
//     }
// }
//
// setupJoinKeys 对应 Go：计算 join key 总长度、是否内联、序列化模式和 keyMode。
// pub fn setup_join_keys(meta: &mut JoinTableMeta, build_key_index: &[i32], build_key_types: &[types::FieldType], probe_key_types: &[types::FieldType]) {
//     meta.is_join_keys_fixed_length = true;
//     meta.join_keys_length = 0;
//     meta.is_join_keys_inlined = true;
//     meta.serialize_modes = Vec::with_capacity(build_key_index.len());
//     let mut is_all_key_integer = true;
//     let mut var_length_key_number = 0;
//     let mut key_index_map = std::collections::HashSet::new();
//     for (index, key_index) in build_key_index.iter().enumerate() {
//         let prop = get_key_prop(&build_key_types[index]);
//         if prop.key_length != chunk::VarElemLen {
//             meta.join_keys_length += prop.key_length;
//         } else {
//             meta.is_join_keys_fixed_length = false;
//             var_length_key_number += 1;
//         }
//         if !prop.can_be_inlined {
//             meta.is_join_keys_inlined = false;
//         }
//         if prop.is_key_integer {
//             let probe_key_prop = get_key_prop(&probe_key_types[index]);
//             if !probe_key_prop.is_key_integer {
//                 panic!("build key is integer but probe key is not integer, should not happen");
//             }
//             if prop.is_key_unsigned != probe_key_prop.is_key_unsigned {
// mixed signed/unsigned integer 需要额外 sign flag，因此不能直接内联原始整数。
//                 meta.serialize_modes.push(codec::SerializeMode::NeedSignFlag);
//                 meta.is_join_keys_inlined = false;
//                 if meta.is_join_keys_fixed_length {
//                     meta.join_keys_length += 1;
//                 }
//             } else {
//                 meta.serialize_modes.push(codec::SerializeMode::Normal);
//             }
//         } else {
//             is_all_key_integer = false;
//             if prop.key_length == chunk::VarElemLen {
// 变长列默认保留长度，否则 [a, aa] 与 [aa, a] 这类组合无法区分。
//                 meta.serialize_modes.push(codec::SerializeMode::KeepVarColumnLength);
//             } else {
//                 meta.serialize_modes.push(codec::SerializeMode::Normal);
//             }
//         }
//         key_index_map.insert(*key_index);
//     }
//     if !meta.is_join_keys_fixed_length {
//         meta.join_keys_length = -1;
//     }
//     if build_key_index.len() != key_index_map.len() {
//         meta.is_join_keys_inlined = false;
//     }
//     if !meta.is_join_keys_inlined && var_length_key_number == 1 {
//         for mode in &mut meta.serialize_modes {
//             if *mode == codec::SerializeMode::KeepVarColumnLength {
//                 *mode = codec::SerializeMode::Normal;
//             }
//         }
//     }
//     if is_all_key_integer && build_key_index.len() == 1 && meta.serialize_modes[0] != codec::SerializeMode::NeedSignFlag {
//         meta.key_mode = KeyMode::OneInt64;
//     } else if meta.is_join_keys_fixed_length {
//         meta.key_mode = KeyMode::FixedSerializedKey;
//     } else {
//         meta.key_mode = KeyMode::VariableSerializedKey;
//     }
// }
//
// setupColumnOrder 对应 Go：按 key、other condition、输出列顺序决定 row_data 保存列顺序。
// pub fn setup_column_order(
//     meta: &mut JoinTableMeta,
//     build_key_index: &[i32],
//     build_types: &[types::FieldType],
//     columns_used_by_other_condition: &[i32],
//     output_columns: Option<Vec<i32>>,
//     saved_column_length: i32,
// ) {
//     meta.row_columns_order = Vec::with_capacity(saved_column_length as usize);
//     meta.columns_size = Vec::with_capacity(saved_column_length as usize);
//     let mut used_column_map = std::collections::HashSet::with_capacity(saved_column_length as usize);
//     if meta.is_join_keys_inlined {
// join key 内联时，key 列必须放在 row layout 最前面，方便 probe 阶段快速比较。
//         for index in build_key_index {
//             update_column_order(meta, &mut used_column_map, build_types, *index);
//         }
//     }
//     meta.column_count_needed_for_other_condition = 0;
//     if !columns_used_by_other_condition.is_empty() {
//         for index in columns_used_by_other_condition {
//             update_column_order(meta, &mut used_column_map, build_types, *index);
//         }
//         meta.column_count_needed_for_other_condition = used_column_map.len() as i32;
//     }
//     if let Some(output_columns) = output_columns {
//         for index in output_columns {
//             update_column_order(meta, &mut used_column_map, build_types, index);
//         }
//     } else {
// outputColumns = nil 表示最终结果需要 build side 全部列。
//         for index in 0..build_types.len() as i32 {
//             update_column_order(meta, &mut used_column_map, build_types, index);
//         }
//     }
// }
//
// fn update_column_order(meta: &mut JoinTableMeta, used_column_map: &mut std::collections::HashSet<i32>, build_types: &[types::FieldType], index: i32) {
//     if used_column_map.insert(index) {
//         meta.row_columns_order.push(index);
//         meta.columns_size.push(chunk::GetFixedLen(&build_types[index as usize]));
//     }
// }
// */
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
