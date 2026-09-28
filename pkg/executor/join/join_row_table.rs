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

// Hash Join V2 构建侧行式存储（`RowTable` / `RowTableSegment`）。
//
// 将构建侧行编码为连续字节布局，并维护 hash、分区下标与同桶 next 指针
// （tagged pointer），供探测阶段沿冲突链遍历。多个 segment 组成完整
// `RowTable`，支持合并、按全局行号取行与有效 join key 位置换算。

// hash join v2 的行式存储段：rawData 保存链表指针、null map、join key 和 row data；
//
// Go 常量对应 unsafe.Sizeof 结果；使用 size_of 保留平台相关语义。
// pub const SIZE_OF_NEXT_PTR: usize = std::mem::size_of::<usize>();
// pub const SIZE_OF_ELEMENT_SIZE: usize = std::mem::size_of::<u32>();
// pub const SIZE_OF_UNSAFE_POINTER: usize = std::mem::size_of::<*const u8>();
// pub const SIZE_OF_UINTPTR: usize = std::mem::size_of::<usize>();
//
// pub static FAKE_ADDR_PLACE_HOLDER: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];
// pub static mut FAKE_ADDR_PLACE_HOLDER_LEN: usize = 8;
// pub static mut USED_FLAG_MASK: u32 = 0;
// pub static mut BIT_MASK_IN_UINT32: [u32; 32] = [0; 32];
//
// init 对应 Go 包初始化：根据当前端序初始化 nullmap 中 used flag 的 mask。
// pub fn init_join_row_table_globals() {
//     let endianness_test: u32 = 1 << 7;
//     let low8_value = unsafe { *(&endianness_test as *const u32 as *const u8) };
//     if u32::from(low8_value) == endianness_test {
// 小端：最低地址保存最低有效字节，byte 内 bit mask 按 Go 原实现从高到低排列。
//         initialize_bit_masks(true);
//     } else {
// 大端：最低地址保存最高有效字节，mask 从 bit31 到 bit0 线性排列。
//         initialize_bit_masks(false);
//     }
//     unsafe {
//         USED_FLAG_MASK = BIT_MASK_IN_UINT32[0];
//     }
// }
//
// initializeBitMasks 对应 Go：封装端序差异，让 used flag 与 null map 的 atomic uint32 访问保持一致。
// pub fn initialize_bit_masks(is_little_endian: bool) {
//     for i in 0..32 {
//         unsafe {
//             if is_little_endian {
//                 BIT_MASK_IN_UINT32[i] = 1_u32 << (7 - (i % 8) + (i / 8) * 8);
//             } else {
//                 BIT_MASK_IN_UINT32[i] = 1_u32 << (31 - i);
//             }
//         }
//     }
// }
//
// heapObjectsCanMove 对应 Go //go:linkname runtime.heapObjectsCanMove；这里只保留外部运行时判断入口。
// pub fn heap_objects_can_move() -> bool {
//     runtime::heapObjectsCanMove()
// }
//
// RowTableSegment 对应 Go rowTableSegment，保存一段连续行数据及其 hash、起始偏移和可插入 hash table 的行号。
// pub struct RowTableSegment {
//     /*
//        Go 行布局：
//        | next_row_ptr | null_map | serialized_key/key_length | row_data |
//        next_row_ptr 用于 hash table 同 hash value 链表；
//        null_map 可选，包含列 NULL 标记和右半/外连接使用的 used flag；
//        serialized_key/key_length 可选，保存非内联 key 或变长 key 长度；
//        row_data 保存 build side 输出列，变长列按 size + raw data 顺序写入。
//        Rust 保留顺序访问设计，不尝试改写为安全随机访问结构。
//     */
//     pub raw_data: Vec<u8>,
//     pub hash_values: Vec<u64>,
//     pub row_start_offset: Vec<u64>,
//     pub valid_join_key_pos: Vec<i32>,
// taggedBits 表示 rawData 指针最高位可用于 tag 的位数，Go 用 MSB 存 hash tag。
//     pub tagged_bits: u8,
// }
//
// impl RowTableSegment {
// totalUsedBytes 对应 Go：按 capacity 估算本 segment 占用。
//     pub fn total_used_bytes(&self) -> i64 {
//         let mut ret = self.raw_data.capacity() as i64;
//         ret += (self.hash_values.capacity() * serialization::Uint64Len as usize) as i64;
//         ret += (self.row_start_offset.capacity() * serialization::Uint64Len as usize) as i64;
//         ret += (self.valid_join_key_pos.capacity() * serialization::IntLen as usize) as i64;
//         ret
//     }
//
// getRowPointer 对应 Go：返回 rawData 指定行起始地址，属于 unsafe 行式存储入口。
//     pub fn get_row_pointer(&self, index: usize) -> *mut u8 {
//         unsafe { self.raw_data.as_ptr().add(self.row_start_offset[index] as usize) as *mut u8 }
//     }
//
// initTaggedBits 对应 Go：用首尾行地址共同计算可用 tag bits。
//     pub fn init_tagged_bits(&mut self) {
//         let start_ptr = self.get_row_pointer(0) as usize;
//         let end_ptr = self.get_row_pointer(self.row_start_offset.len() - 1) as usize;
//         self.tagged_bits = getTaggedBitsFromUintptr(end_ptr | start_ptr);
//     }
//
// rowCount/validKeyCount/getRowNum 对应 Go 简单计数方法。
//     pub fn row_count(&self) -> i64 {
//         self.row_start_offset.len() as i64
//     }
//
//     pub fn valid_key_count(&self) -> u64 {
//         self.valid_join_key_pos.len() as u64
//     }
//
//     pub fn get_row_num(&self) -> usize {
//         self.hash_values.len()
//     }
//
// getRowBytes 对应 Go：最后一行切到 rawData 末尾，其余切到下一行起始 offset。
//     pub fn get_row_bytes(&self, idx: usize) -> &[u8] {
//         let row_num = self.get_row_num();
//         let start = self.row_start_offset[idx] as usize;
//         if idx == row_num - 1 {
//             &self.raw_data[start..]
//         } else {
//             let end = self.row_start_offset[idx + 1] as usize;
//             &self.raw_data[start..end]
//         }
//     }
// }
//
// newRowTableSegment 对应 Go 构造函数。
// pub fn new_row_table_segment() -> RowTableSegment {
//     RowTableSegment {
//         raw_data: Vec::new(),
//         hash_values: Vec::new(),
//         row_start_offset: Vec::new(),
//         valid_join_key_pos: Vec::new(),
//         tagged_bits: 0,
//     }
// }
//
// setNextRowAddress 对应 Go：把 taggedPtr 写入 row 起始地址，链接同 hash bucket 的下一行。
// pub fn set_next_row_address(row_start: *mut u8, next_row_address: TaggedPtr) {
//     unsafe {
//         *(row_start as *mut TaggedPtr) = next_row_address;
//     }
// }
//
// getNextRowAddress 对应 Go：读取 next ptr 后校验 hash tag，不匹配表示链表结束。
// pub fn get_next_row_address(row_start: *mut u8, tag_helper: &TagPtrHelper, hash_value: u64) -> TaggedPtr {
//     let ret = unsafe { *(row_start as *const TaggedPtr) };
//     let hash_tag_value = tag_helper.getTaggedValue(hash_value);
//     if (ret as u64) & hash_tag_value != hash_tag_value {
//         return 0;
//     }
//     ret
// }
//
// RowTable 对应 Go rowTable，按多个 segment 组成完整 build side row table。
// pub struct RowTable {
//     pub segments: Vec<RowTableSegment>,
// }
//
// impl RowTable {
// getTotalMemoryUsage 对应 Go：聚合所有 segment 的 capacity 估算。
//     pub fn get_total_memory_usage(&self) -> i64 {
//         self.segments.iter().map(|seg| seg.total_used_bytes()).sum()
//     }
//
// getSegments/clearSegments 对应 Go：暴露与清空 segment 列表。
//     pub fn get_segments(&self) -> &[RowTableSegment] {
//         &self.segments
//     }
//
//     pub fn clear_segments(&mut self) {
//         self.segments.clear();
//     }
//
// getRowPointer 对应 Go 测试辅助函数：按全局 rowIndex 找到 segment 内地址。
//     pub fn get_row_pointer(&self, mut row_index: usize) -> *mut u8 {
//         for seg in &self.segments {
//             if row_index < seg.row_start_offset.len() {
//                 return seg.get_row_pointer(row_index);
//             }
//             row_index -= seg.row_start_offset.len();
//         }
//         std::ptr::null_mut()
//     }
//
// getValidJoinKeyPos 对应 Go：把 validJoinKeyPos 的 segment 局部位置换算为全局 row 位置。
//     pub fn get_valid_join_key_pos(&self, mut row_index: usize) -> i32 {
//         let mut start_offset = 0_i32;
//         for seg in &self.segments {
//             if row_index < seg.valid_join_key_pos.len() {
//                 return start_offset + seg.valid_join_key_pos[row_index];
//             }
//             row_index -= seg.valid_join_key_pos.len();
//             start_offset += seg.row_start_offset.len() as i32;
//         }
//         -1
//     }
//
// merge 对应 Go：追加另一个 rowTable 的所有 segments。
//     pub fn merge(&mut self, other: RowTable) {
//         self.segments.extend(other.segments);
//     }
//
// rowCount/validKeyCount 对应 Go 聚合计数。
//     pub fn row_count(&self) -> u64 {
//         self.segments.iter().map(|s| s.row_count() as u64).sum()
//     }
//
//     pub fn valid_key_count(&self) -> u64 {
//         self.segments.iter().map(|s| s.valid_key_count()).sum()
//     }
// }
//
// newRowTable 对应 Go 构造函数。
// pub fn new_row_table() -> RowTable {
//     RowTable { segments: Vec::new() }
// }
// */
use crate::join_table_meta::EncodedRow;
use crate::tagged_ptr::{TagPtrHelper, TaggedPtr};

/// 同桶 next 指针字段宽度（`usize`）。
pub const SIZE_OF_NEXT_PTR: usize = std::mem::size_of::<usize>();
/// 元素长度字段宽度（`u32`），对应 Go `SizeOfElementSize`。
pub const SIZE_OF_ELEMENT_SIZE: usize = std::mem::size_of::<u32>();

/// 一段连续的构建侧行数据及其 hash / 分区 / 冲突链 next 信息。
#[derive(Clone, Debug, Default)]
pub struct RowTableSegment {
    /// 原始字节缓冲（保留 Go rawData 语义占位）。
    pub raw_data: Vec<u8>,
    /// 已解码/编码的行列表。
    pub rows: Vec<EncodedRow>,
    /// 每行 join key 的 hash 值。
    pub hash_values: Vec<u64>,
    /// 每行所属分区下标。
    pub partition_indices: Vec<usize>,
    /// 可插入哈希表的有效 key 行数（跳过 NULL key）。
    pub valid_key_count: u64,
    /// 可插入哈希表的具体行下标，对应 Go `validJoinKeyPos`。
    pub valid_join_key_positions: Vec<usize>,
    /// 指针高位可用于 hash tag 的位数。
    pub tagged_bits: u8,
    /// 同 hash 桶冲突链的 next tagged pointer。
    pub next_rows: Vec<Option<TaggedPtr>>,
}
impl RowTableSegment {
    /// 按各字段 capacity 估算本 segment 占用字节数。
    pub fn total_used_bytes(&self) -> i64 {
        (self.raw_data.capacity()
            + self
                .rows
                .iter()
                .map(|row| row.bytes.capacity() + row.null_map.capacity())
                .sum::<usize>()
            + self.hash_values.capacity() * 8
            + self.partition_indices.capacity() * std::mem::size_of::<usize>()
            + self.valid_join_key_positions.capacity() * std::mem::size_of::<usize>()
            + self.next_rows.capacity() * SIZE_OF_NEXT_PTR) as i64
    }
    /// 本段行数。
    pub fn row_count(&self) -> i64 {
        self.rows.len() as i64
    }
    /// 有效 join key 数。
    pub fn valid_key_count(&self) -> u64 {
        self.valid_key_count
    }
    /// 按段内下标取编码行。
    pub fn get_row(&self, index: usize) -> Option<&EncodedRow> {
        self.rows.get(index)
    }
    /// 取编码行原始字节切片。
    pub fn get_row_bytes(&self, index: usize) -> Option<&[u8]> {
        self.rows.get(index).map(|row| row.bytes.as_slice())
    }
    /// 根据行向量基址计算可用 tagged bits。
    pub fn init_tagged_bits(&mut self) {
        let address = self.rows.as_ptr() as usize;
        self.tagged_bits = crate::tagged_ptr::get_tagged_bits_from_uintptr(address);
    }
    /// 设置冲突链下一行的 tagged pointer。
    pub fn set_next_row_address(&mut self, row_index: usize, next: Option<TaggedPtr>) {
        if self.next_rows.len() <= row_index {
            self.next_rows.resize(row_index + 1, None);
        }
        self.next_rows[row_index] = next;
    }
    /// 读取 next 指针并校验 hash tag；tag 不匹配视为链表结束。
    pub fn get_next_row_address(
        &self,
        row_index: usize,
        helper: &TagPtrHelper,
        hash_value: u64,
    ) -> Option<TaggedPtr> {
        let pointer = self.next_rows.get(row_index).copied().flatten()?;
        // 高位 tag 与当前 hash 的 tag 不一致则视为链尾。
        (pointer as u64 & helper.tagged_mask == helper.get_tagged_value(hash_value))
            .then_some(pointer)
    }
}

/// 由多个 `RowTableSegment` 组成的完整构建侧行表。
#[derive(Clone, Debug, Default)]
pub struct RowTable {
    segments: Vec<RowTableSegment>,
}
impl RowTable {
    /// 聚合全部 segment 的 capacity 内存估算。
    pub fn total_memory_usage(&self) -> i64 {
        self.segments
            .iter()
            .map(RowTableSegment::total_used_bytes)
            .sum()
    }
    /// 只读访问 segment 列表。
    pub fn segments(&self) -> &[RowTableSegment] {
        &self.segments
    }
    /// 可变访问 segment 列表。
    pub fn segments_mut(&mut self) -> &mut Vec<RowTableSegment> {
        &mut self.segments
    }
    /// 清空全部 segment（spill 释放）。
    pub fn clear_segments(&mut self) {
        self.segments.clear();
    }
    /// 按全局行号跨 segment 取行。
    pub fn get_row(&self, mut row_index: usize) -> Option<&EncodedRow> {
        for segment in &self.segments {
            if row_index < segment.rows.len() {
                return segment.rows.get(row_index);
            }
            row_index -= segment.rows.len();
        }
        None
    }
    /// 将有效 join key 序号换算为跨 segment 的全局行号，对应 Go
    /// `getValidJoinKeyPos`。
    pub fn valid_join_key_position(&self, mut row_index: usize) -> Option<usize> {
        let mut row_offset = 0;
        for segment in &self.segments {
            let valid_count = segment.valid_key_count as usize;
            if row_index < valid_count {
                let local_position = segment
                    .valid_join_key_positions
                    .get(row_index)
                    .copied()
                    // Compatibility for old fixtures whose valid rows are a prefix.
                    .or_else(|| {
                        segment
                            .valid_join_key_positions
                            .is_empty()
                            .then_some(row_index)
                    })?;
                return Some(row_offset + local_position);
            }
            row_index -= valid_count;
            row_offset += segment.rows.len();
        }
        None
    }
    /// 追加另一个 `RowTable` 的全部 segments。
    pub fn merge(&mut self, mut other: RowTable) {
        self.segments.append(&mut other.segments);
    }
    /// 全部行数。
    pub fn row_count(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.rows.len() as u64)
            .sum()
    }
    /// 全部有效 key 数。
    pub fn valid_key_count(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.valid_key_count)
            .sum()
    }
}
