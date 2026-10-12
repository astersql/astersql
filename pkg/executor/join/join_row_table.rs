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
