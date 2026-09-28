// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Hash Join V2 的分区哈希表（`HashTableV2` / `SubTable`）。
//
// 将构建侧 `RowTable` 按分区建成哈希桶，支持 lookup（探测查找）、
// 分区替换/清空、行位置迭代（`RowIter`）以及哈希表容量与内存估算。
// tagged pointer（带标签指针）相关语义在 Go 桩代码中保留，Rust 侧用
// `(hash, RowPos)` 桶列表表达冲突链。

// V2 subTable/taggedPtr 哈希桶、row iterator 和哈希表内存估算。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// #![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]
// use std::any::Any;
// use std::collections::HashMap;
// subTable 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct subTable {
//     pub rowData: Option<Box<rowTable>>,
// the taggedPtr is used to save the row address, during hash join build stage
// it will convert the chunk data into row format, each row there is an unsafe.Pointer
// pointing the start address of the row. The unsafe.Pointer will be converted to
// taggedPtr and saved in hashTable.
// Generally speaking it is unsafe or even illegal in go to save unsafe.Pointer
// into uintptr, and later convert uintptr back to unsafe.Pointer since after save
// the value of unsafe.Pointer into uintptr, it has no pointer semantics, and may
// become invalid after GC. But it is ok to do this in hash join so far because
// 1. the check of heapObjectsCanMove makes sure that if the object is in heap, the address will not be changed after GC
// 2. row address only points to a valid address in `rowTableSegment.rawData`. `rawData` is a slice in `rowTableSegment`, and it will be used by multiple goroutines,
//    and its size will be runtime expanded, this kind of slice will always be allocated in heap
//     pub hashTable: Vec<taggedPtr>,
//     pub posMask: u64,
//     pub isRowTableEmpty: bool,
//     pub isHashTableEmpty: bool,
// }
// getTotalMemoryUsage 对应 Go 声明 `func (st *subTable) getTotalMemoryUsage() int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl subTable {
//     pub fn getTotalMemoryUsage(&mut self) -> i64 {
//     return self.rowData.getTotalMemoryUsage() + getHashTableMemoryUsage(uint64(len(self.hashTable)))
// }
// }
// lookup 对应 Go 声明 `func (st *subTable) lookup(hashValue uint64, tagHelper *tagPtrHelper) taggedPtr {`；保留原控制流、错误处理和外部依赖调用形状。
// impl subTable {
//     pub fn lookup(&mut self, hashValue: u64, tagHelper: Option<Box<tagPtrHelper>>) -> taggedPtr {
//     ret = self.hashTable[hashValue&self.posMask]
//     hashTagValue = tagHelper.getTaggedValue(hashValue)
//     if uint64(ret)&hashTagValue != hashTagValue {
// if tag value not match, the key will not be matched
//         return 0
//     }
//     return ret
// }
// }
// nextPowerOfTwo 对应 Go 声明 `func nextPowerOfTwo(value uint64) uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn nextPowerOfTwo(value: u64) -> u64 {
//     ret = uint64(2)
//     round = 1
//     for ; ret <= value && round <= 64; ret = ret << 1 {
//         round++
//     }
//     if round > 64 {
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         panic("input value is too large")
//     }
//     return ret
// }
// newSubTable 对应 Go 声明 `func newSubTable(table *rowTable) *subTable {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn newSubTable(table: Option<Box<rowTable>>) -> Option<Box<subTable>> {
//     ret = &subTable{
//         rowData:          table,
//         isHashTableEmpty: false,
//         isRowTableEmpty:  false,
//     }
//     if table.rowCount() == 0 {
//         ret.isRowTableEmpty = true
//     }
//     if table.validKeyCount() == 0 {
//         ret.isHashTableEmpty = true
//     }
//     hashTableLength = max(nextPowerOfTwo(table.validKeyCount()), uint64(32))
//     ret.hashTable = make([]taggedPtr, hashTableLength)
//     ret.posMask = hashTableLength - 1
//     return ret
// }
// updateHashValue 对应 Go 声明 `func (st *subTable) updateHashValue(hashValue uint64, rowAddress unsafe.Pointer, tagHelper *tagPtrHelper) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl subTable {
//     pub fn updateHashValue(&mut self, hashValue: u64, rowAddress: unsafe::Pointer, tagHelper: Option<Box<tagPtrHelper>>) {
//     pos = hashValue & self.posMask
//     prev = self.hashTable[pos]
//     tagValue = tagHelper.getTaggedValue(hashValue | uint64(prev))
//     taggedAddress = tagHelper.toTaggedPtr(tagValue, rowAddress)
//     self.hashTable[pos] = taggedAddress
//     setNextRowAddress(rowAddress, prev)
// }
// }
// atomicUpdateHashValue 对应 Go 声明 `func (st *subTable) atomicUpdateHashValue(hashValue uint64, rowAddress unsafe.Pointer, tagHelper *tagPtrHelper) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl subTable {
//     pub fn atomicUpdateHashValue(&mut self, hashValue: u64, rowAddress: unsafe::Pointer, tagHelper: Option<Box<tagPtrHelper>>) {
//     pos = hashValue & self.posMask
//     for {
// unsafe 指针/地址操作来自 Go 实现，只保留数据流并提示后续审查所有权。
//         prev = taggedPtr(atomic.LoadUintptr((*uintptr)(unsafe.Pointer(&self.hashTable[pos]))))
//         tagValue = tagHelper.getTaggedValue(hashValue | uint64(prev))
//         taggedAddress = tagHelper.toTaggedPtr(tagValue, rowAddress)
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//         if atomic.CompareAndSwapUintptr((*uintptr)(unsafe.Pointer(&self.hashTable[pos])), uintptr(prev), uintptr(taggedAddress)) {
//             setNextRowAddress(rowAddress, prev)
//             break
//         }
//     }
// }
// }
// build 对应 Go 声明 `func (st *subTable) build(startSegmentIndex int, endSegmentIndex int, tagHelper *tagPtrHelper) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl subTable {
//     pub fn build(&mut self, startSegmentIndex: i32, endSegmentIndex: i32, tagHelper: Option<Box<tagPtrHelper>>) {
//     if startSegmentIndex == 0 && endSegmentIndex == len(self.rowData.segments) {
//         for i = startSegmentIndex; i < endSegmentIndex; i++ {
//             for _, index = range self.rowData.segments[i].validJoinKeyPos {
//                 rowAddress = self.rowData.segments[i].getRowPointer(index)
//                 hashValue = self.rowData.segments[i].hashValues[index]
//                 self.updateHashValue(hashValue, rowAddress, tagHelper)
//             }
//         }
//     } else {
//         for i = startSegmentIndex; i < endSegmentIndex; i++ {
//             for _, index = range self.rowData.segments[i].validJoinKeyPos {
//                 rowAddress = self.rowData.segments[i].getRowPointer(index)
//                 hashValue = self.rowData.segments[i].hashValues[index]
//                 self.atomicUpdateHashValue(hashValue, rowAddress, tagHelper)
//             }
//         }
//     }
// }
// }
// hashTableV2 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct hashTableV2 {
//     pub tables: Vec<Option<Box<subTable>>>,
//     pub partitionNumber: u64,
// }
// getPartitionMemoryUsage 对应 Go 声明 `func (ht *hashTableV2) getPartitionMemoryUsage(partID int) int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn getPartitionMemoryUsage(&mut self, partID: i32) -> i64 {
//     if self.tables[partID] != None {
//         return self.tables[partID].getTotalMemoryUsage()
//     }
//     return 0
// }
// }
// clearPartitionSegments 对应 Go 声明 `func (ht *hashTableV2) clearPartitionSegments(partID int) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn clearPartitionSegments(&mut self, partID: i32) {
//     if self.tables[partID] != None {
//         self.tables[partID].rowData.clearSegments()
//         self.tables[partID].hashTable = None
//     }
// }
// }
// rowPos 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct rowPos {
//     pub subTableIndex: i32,
//     pub rowSegmentIndex: i32,
//     pub rowIndex: u64,
// }
// rowIter 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct rowIter {
//     pub table: Option<Box<hashTableV2>>,
//     pub currentPos: Option<Box<rowPos>>,
//     pub endPos: Option<Box<rowPos>>,
// }
// getValue 对应 Go 声明 `func (ri *rowIter) getValue() unsafe.Pointer {`；保留原控制流、错误处理和外部依赖调用形状。
// impl rowIter {
//     pub fn getValue(&mut self) -> unsafe::Pointer {
//     return self.table.tables[self.currentPos.subTableIndex].rowData.segments[self.currentPos.rowSegmentIndex].getRowPointer(int(self.currentPos.rowIndex))
// }
// }
// next 对应 Go 声明 `func (ri *rowIter) next() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl rowIter {
//     pub fn next(&mut self) {
//     self.currentPos.rowIndex++
//     if self.currentPos.rowIndex == uint64(self.table.tables[self.currentPos.subTableIndex].rowData.segments[self.currentPos.rowSegmentIndex].rowCount()) {
//         self.currentPos.rowSegmentIndex++
//         self.currentPos.rowIndex = 0
//         for self.currentPos.rowSegmentIndex == len(self.table.tables[self.currentPos.subTableIndex].rowData.segments) {
//             self.currentPos.subTableIndex++
//             self.currentPos.rowSegmentIndex = 0
//             if self.currentPos.subTableIndex == int(self.table.partitionNumber) {
//                 break
//             }
//         }
//     }
// }
// }
// isEnd 对应 Go 声明 `func (ri *rowIter) isEnd() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl rowIter {
//     pub fn isEnd(&mut self) -> bool {
//     return !(self.currentPos.subTableIndex < self.endPos.subTableIndex || self.currentPos.rowSegmentIndex < self.endPos.rowSegmentIndex || self.currentPos.rowIndex < self.endPos.rowIndex)
// }
// }
// newJoinHashTableForTest 对应 Go 声明 `func newJoinHashTableForTest(partitionedRowTables []*rowTable) *hashTableV2 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn newJoinHashTableForTest(partitionedRowTables: Vec<Option<Box<rowTable>>>) -> Option<Box<hashTableV2>> {
// first make sure there is no None rowTable
//     jht = &hashTableV2{
//         tables:          make([]*subTable, len(partitionedRowTables)),
//         partitionNumber: uint64(len(partitionedRowTables)),
//     }
//     for i, rowTable = range partitionedRowTables {
//         jht.tables[i] = newSubTable(rowTable)
//     }
//     return jht
// }
// createRowPos 对应 Go 声明 `func (ht *hashTableV2) createRowPos(pos uint64) *rowPos {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn createRowPos(&mut self, pos: u64) -> Option<Box<rowPos>> {
//     if pos > self.totalRowCount() {
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         panic("invalid call to createRowPos, the input pos should be in [0, totalRowCount]")
//     }
//     if pos == self.totalRowCount() {
//         return &rowPos{
//             subTableIndex:   len(self.tables),
//             rowSegmentIndex: 0,
//             rowIndex:        0,
//         }
//     }
//     subTableIndex = 0
//     for pos >= self.tables[subTableIndex].rowData.rowCount() {
//         pos -= self.tables[subTableIndex].rowData.rowCount()
//         subTableIndex++
//     }
//     rowSegmentIndex = 0
//     for pos >= uint64(self.tables[subTableIndex].rowData.segments[rowSegmentIndex].rowCount()) {
//         pos -= uint64(self.tables[subTableIndex].rowData.segments[rowSegmentIndex].rowCount())
//         rowSegmentIndex++
//     }
//     return &rowPos{
//         subTableIndex:   subTableIndex,
//         rowSegmentIndex: rowSegmentIndex,
//         rowIndex:        pos,
//     }
// }
// }
// createRowIter 对应 Go 声明 `func (ht *hashTableV2) createRowIter(start, end uint64) *rowIter {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn createRowIter(&mut self, start: _, end: u64) -> Option<Box<rowIter>> {
//     if start > end {
//         start = end
//     }
//     return &rowIter{
//         table:      ht,
//         currentPos: self.createRowPos(start),
//         endPos:     self.createRowPos(end),
//     }
// }
// }
// isHashTableEmpty 对应 Go 声明 `func (ht *hashTableV2) isHashTableEmpty() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn isHashTableEmpty(&mut self) -> bool {
//     for _, subTable = range self.tables {
//         if !subTable.isHashTableEmpty {
//             return false
//         }
//     }
//     return true
// }
// }
// totalRowCount 对应 Go 声明 `func (ht *hashTableV2) totalRowCount() uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableV2 {
//     pub fn totalRowCount(&mut self) -> u64 {
//     ret = uint64(0)
//     for _, table = range self.tables {
//         ret += table.rowData.rowCount()
//     }
//     return ret
// }
// }
// getHashTableLengthByRowTable 对应 Go 声明 `func getHashTableLengthByRowTable(table *rowTable) uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn getHashTableLengthByRowTable(table: Option<Box<rowTable>>) -> u64 {
//     return getHashTableLengthByRowLen(table.validKeyCount())
// }
// getHashTableLengthByRowLen 对应 Go 声明 `func getHashTableLengthByRowLen(rowLen uint64) uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn getHashTableLengthByRowLen(rowLen: u64) -> u64 {
//     return max(nextPowerOfTwo(rowLen), uint64(minimalHashTableLen))
// }
// getHashTableMemoryUsage 对应 Go 声明 `func getHashTableMemoryUsage(hashTableLength uint64) int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn getHashTableMemoryUsage(hashTableLength: u64) -> i64 {
//     return int64(hashTableLength) * taggedPointerLen
// }
// */
use crate::join_row_table::RowTable;
use crate::join_table_meta::EncodedRow;

/// 哈希表最小桶数（与 Go `minimalHashTableLen` 对齐）。
pub const MINIMAL_HASH_TABLE_LEN: u64 = 32;
/// 单个 tagged pointer 占用字节数，用于估算哈希表内存。
pub const TAGGED_POINTER_LEN: i64 = std::mem::size_of::<usize>() as i64;

/// 行在分区哈希表中的三维坐标：子表 / segment / 行内下标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowPos {
    /// 分区（subTable）下标。
    pub sub_table_index: usize,
    /// 该分区内 `RowTableSegment` 下标。
    pub row_segment_index: usize,
    /// segment 内行下标。
    pub row_index: usize,
}

/// 单个分区的子哈希表：持有构建侧行数据与按 hash 分桶的冲突链。
#[derive(Clone, Debug)]
pub struct SubTable {
    /// 本分区的行式构建侧数据。
    pub row_data: RowTable,
    /// 哈希桶；每桶为 `(完整 hash, RowPos)` 列表，表达冲突链。
    hash_table: Vec<Vec<(u64, RowPos)>>,
    /// 桶下标掩码，等于 `hash_table.len() - 1`（长度为 2 的幂）。
    pos_mask: u64,
    /// 行表是否为空。
    pub is_row_table_empty: bool,
    /// 哈希表是否无有效 key。
    pub is_hash_table_empty: bool,
}

impl SubTable {
    /// 由分区 `RowTable` 构造子表，并立即 build 全部有效 key。
    pub fn new(row_data: RowTable, partition: usize) -> Self {
        let valid_keys = row_data.valid_key_count();
        let length = get_hash_table_length_by_row_len(valid_keys) as usize;
        Self {
            is_row_table_empty: row_data.row_count() == 0,
            is_hash_table_empty: valid_keys == 0,
            row_data,
            hash_table: vec![Vec::new(); length],
            pos_mask: length as u64 - 1,
        }
        .with_built_rows(partition)
    }
    /// 构造后对全部 segment 执行 build。
    fn with_built_rows(mut self, partition: usize) -> Self {
        self.build(partition, 0, self.row_data.segments().len());
        self
    }
    /// 估算本子表占用：与 Go 一致，只计行数据与 tagged-pointer 桶数组。
    pub fn total_memory_usage(&self) -> i64 {
        self.row_data.total_memory_usage()
            + get_hash_table_memory_usage(self.hash_table.len() as u64)
    }
    /// 按 hash 查找冲突链中完整 hash 匹配的全部 `RowPos`。
    pub fn lookup(&self, hash_value: u64) -> Vec<RowPos> {
        if self.hash_table.is_empty() {
            return Vec::new();
        }
        self.hash_table[(hash_value & self.pos_mask) as usize]
            .iter()
            .filter(|(hash, _)| *hash == hash_value)
            .map(|(_, position)| *position)
            .collect()
    }
    /// 将一行插入对应哈希桶（非原子路径，用于单线程整表 build）。
    pub fn update_hash_value(&mut self, hash_value: u64, position: RowPos) {
        self.hash_table[(hash_value & self.pos_mask) as usize].push((hash_value, position));
        self.is_hash_table_empty = false;
    }
    /// 并发 build 路径入口；当前 Rust 实现委托给非原子更新。
    pub fn atomic_update_hash_value(&mut self, hash_value: u64, position: RowPos) {
        self.update_hash_value(hash_value, position);
    }
    /// 将 `[start_segment, end_segment)` 内有效 join key 写入哈希桶。
    pub fn build(&mut self, partition: usize, start_segment: usize, end_segment: usize) {
        for segment_index in start_segment..end_segment.min(self.row_data.segments().len()) {
            let segment = &self.row_data.segments()[segment_index];
            // Go 按 validJoinKeyPos 的具体下标索引；旧夹具未保存下标时才兼容前缀布局。
            let rows: Vec<(usize, u64)> = if segment.valid_join_key_positions.len()
                == segment.valid_key_count as usize
            {
                segment
                    .valid_join_key_positions
                    .iter()
                    .filter_map(|&row_index| {
                        segment
                            .hash_values
                            .get(row_index)
                            .copied()
                            .map(|hash| (row_index, hash))
                    })
                    .collect()
            } else {
                segment
                    .hash_values
                    .iter()
                    .copied()
                    .take(
                        segment
                            .valid_key_count
                            .min(segment.hash_values.len() as u64) as usize,
                    )
                    .enumerate()
                    .collect()
            };
            for (row_index, hash_value) in rows {
                self.update_hash_value(
                    hash_value,
                    RowPos {
                        sub_table_index: partition,
                        row_segment_index: segment_index,
                        row_index,
                    },
                );
            }
        }
    }
    /// 清空行段与哈希桶；与 Go 一样保留构造时计算的空标志。
    pub fn clear_segments(&mut self) {
        self.row_data.clear_segments();
        self.hash_table.clear();
    }
}

/// Hash Join V2 顶层哈希表：按分区持有若干 `SubTable`。
#[derive(Clone, Debug, Default)]
pub struct HashTableV2 {
    /// 各分区子表；`None` 表示该分区尚未构建或已清空。
    tables: Vec<Option<SubTable>>,
    /// 分区数。
    partition_number: usize,
}

impl HashTableV2 {
    /// 由各分区 `RowTable` 一次性构建完整哈希表。
    pub fn new(partitioned_row_tables: Vec<RowTable>) -> Self {
        let partition_number = partitioned_row_tables.len();
        let tables = partitioned_row_tables
            .into_iter()
            .enumerate()
            .map(|(partition, table)| Some(SubTable::new(table, partition)))
            .collect();
        Self {
            tables,
            partition_number,
        }
    }
    /// 创建空壳表（分区槽位均为 `None`），供后续 `replace_partition` 填充。
    pub fn empty(partition_number: usize) -> Self {
        Self {
            tables: vec![None; partition_number],
            partition_number,
        }
    }
    /// 用新的 `RowTable` 替换指定分区并重建其子哈希表。
    pub fn replace_partition(&mut self, partition: usize, table: RowTable) -> Result<(), String> {
        if partition >= self.partition_number {
            return Err(format!("partition {partition} is out of bounds"));
        }
        self.tables[partition] = Some(SubTable::new(table, partition));
        Ok(())
    }
    /// 在指定分区内按 hash 查找匹配行位置。
    pub fn lookup(&self, partition: usize, hash_value: u64) -> Vec<RowPos> {
        self.tables
            .get(partition)
            .and_then(Option::as_ref)
            .map(|table| table.lookup(hash_value))
            .unwrap_or_default()
    }
    /// 按 `RowPos` 取出编码行（`EncodedRow`）。
    pub fn get_row(&self, position: RowPos) -> Option<&EncodedRow> {
        self.tables
            .get(position.sub_table_index)?
            .as_ref()?
            .row_data
            .segments()
            .get(position.row_segment_index)?
            .get_row(position.row_index)
    }
    /// 单分区内存估算。
    pub fn partition_memory_usage(&self, partition: usize) -> i64 {
        self.tables
            .get(partition)
            .and_then(Option::as_ref)
            .map(SubTable::total_memory_usage)
            .unwrap_or(0)
    }
    /// 全部分区内存之和。
    pub fn total_memory_usage(&self) -> i64 {
        (0..self.partition_number)
            .map(|partition| self.partition_memory_usage(partition))
            .sum()
    }
    /// 清空指定分区的行段与哈希桶（spill 释放）。
    pub fn clear_partition_segments(&mut self, partition: usize) {
        if let Some(Some(table)) = self.tables.get_mut(partition) {
            table.clear_segments();
        }
    }
    /// 是否所有分区哈希表均为空。
    pub fn is_hash_table_empty(&self) -> bool {
        self.tables
            .iter()
            .all(|table| table.as_ref().is_none_or(|table| table.is_hash_table_empty))
    }
    /// 全部行数（跨分区求和）。
    pub fn total_row_count(&self) -> u64 {
        self.tables
            .iter()
            .filter_map(Option::as_ref)
            .map(|table| table.row_data.row_count())
            .sum()
    }
    /// 将全局行序号映射为 `RowPos`；`position == total` 时返回越界哨兵位置。
    pub fn create_row_pos(&self, mut position: u64) -> Result<RowPos, String> {
        if position > self.total_row_count() {
            return Err("row position exceeds total row count".into());
        }
        // 等于总行数时返回“结束”哨兵，供迭代器上界使用。
        if position == self.total_row_count() {
            return Ok(RowPos {
                sub_table_index: self.partition_number,
                row_segment_index: 0,
                row_index: 0,
            });
        }
        // 依次扣减各分区/segment 行数，定位到具体坐标。
        for (partition, table) in self.tables.iter().enumerate() {
            let Some(table) = table else { continue };
            if position >= table.row_data.row_count() {
                position -= table.row_data.row_count();
                continue;
            }
            for (segment, data) in table.row_data.segments().iter().enumerate() {
                if position < data.row_count() as u64 {
                    return Ok(RowPos {
                        sub_table_index: partition,
                        row_segment_index: segment,
                        row_index: position as usize,
                    });
                }
                position -= data.row_count() as u64;
            }
        }
        Err("row position could not be resolved".into())
    }
    /// 创建覆盖全局行区间 `[start, end)` 的行迭代器。
    pub fn create_row_iter(&self, start: u64, end: u64) -> Result<RowIter<'_>, String> {
        RowIter::new(self, start.min(end), end)
    }
}

/// 跨分区扫描构建侧行的迭代器。
pub struct RowIter<'a> {
    table: &'a HashTableV2,
    current: u64,
    end: u64,
}
impl<'a> RowIter<'a> {
    /// 校验上界不超过总行数后构造迭代器。
    fn new(table: &'a HashTableV2, current: u64, end: u64) -> Result<Self, String> {
        if end > table.total_row_count() {
            return Err("iterator end exceeds total row count".into());
        }
        Ok(Self {
            table,
            current,
            end,
        })
    }
    /// 取当前全局序号对应的编码行。
    pub fn get_value(&self) -> Option<&'a EncodedRow> {
        let position = self.table.create_row_pos(self.current).ok()?;
        self.table.get_row(position)
    }
    /// 是否已到达结束位置。
    pub fn is_end(&self) -> bool {
        self.current >= self.end
    }
}
impl Iterator for RowIter<'_> {
    type Item = RowPos;
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_end() {
            return None;
        }
        let position = self.table.create_row_pos(self.current).ok()?;
        self.current += 1;
        Some(position)
    }
}

/// 返回严格大于 `value` 的最小 2 的幂；过大则 panic。
pub fn next_power_of_two(value: u64) -> u64 {
    if value >= 1_u64 << 63 {
        panic!("input value is too large");
    }
    let mut result = 2_u64;
    while result <= value {
        result <<= 1;
    }
    result
}
/// 按 `RowTable` 有效 key 数计算哈希表桶长度。
pub fn get_hash_table_length_by_row_table(table: &RowTable) -> u64 {
    get_hash_table_length_by_row_len(table.valid_key_count())
}
/// 按有效行数取不小于 `MINIMAL_HASH_TABLE_LEN` 的 2 的幂桶长。
pub fn get_hash_table_length_by_row_len(row_len: u64) -> u64 {
    next_power_of_two(row_len).max(MINIMAL_HASH_TABLE_LEN)
}
/// 按桶数量估算 tagged pointer 数组占用的字节数。
pub fn get_hash_table_memory_usage(hash_table_length: u64) -> i64 {
    hash_table_length
        .saturating_mul(TAGGED_POINTER_LEN as u64)
        .min(i64::MAX as u64) as i64
}
