// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 内存缓冲读取器：UnionScan 场景下读取事务未提交写缓冲中的行。
//
// 在事务（Transaction）内，已修改但尚未提交的数据存放在内存写缓冲（mem buffer）。
// UnionScan 需要把缓冲中的变更与快照（Snapshot）结果合并，避免读到过期视图。
// 本模块提供表扫描、索引扫描、IndexLookUp、IndexMerge 等内存侧读取器，
// 以及将事务缓冲与临时/缓存快照按 key 合并的迭代器。

// 事务内存缓冲（MemBuffer）上的行/索引读取。
//
// 供 UnionScan 等算子合并「已提交快照」与「当前事务未提交写」：
// 在内存键值缓冲上解码索引项、表行与 Handle（行定位符），并支持
// IndexLookUp / IndexMerge 等组合路径。

#![allow(non_camel_case_types, non_snake_case)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// 编码后的键字节。
/// 编码后的键字节。
pub type Key = Vec<u8>;
/// 编码后的值字节；空值表示删除标记。
/// 编码后的值字节。
pub type Value = Vec<u8>;
/// 一行逻辑列值。
/// 一行 Datum 序列。
pub type Row = Vec<Datum>;
/// 遍历 KV 时的回调：接收 key/value，可提前报错中止。
/// 遍历键值对时的回调类型。
pub type processKVFunc<'a> = dyn FnMut(&[u8], &[u8]) -> Result<(), MemReaderError> + 'a;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单元格标量：空、有符号/无符号整数、字节或文本。
/// 内存行中的标量：空、有/无符号整数、字节、文本。
pub enum Datum {
    Null,
    Signed(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// 行句柄（Handle）：定位一行的主键标识；分区表可嵌套物理分区 ID。
/// 行 Handle：整型主键、公共句柄（Common Handle）或分区包装。
/// Common Handle：以多列主键编码为键的定位方式。
pub enum Handle {
    Int(i64),
    Common(Vec<u8>),
    Partition {
        partition_id: i64,
        inner: Box<Handle>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 半开区间 [start, end) 的键范围扫描描述。
/// 半开键区间 [start, end)。
pub struct KeyRange {
    pub start: Key,
    pub end: Key,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 一对键值。
/// 一对键值。
pub struct KvPair {
    pub key: Key,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 字段类型摘要：当前仅保留是否无符号。
/// 字段类型摘要（此处仅保留 unsigned）。
pub struct FieldType {
    pub unsigned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 列元信息：ID、偏移、主键属性与是否需从句柄还原数据。
/// 列元信息：ID、偏移、是否主键、是否需还原数据等。
pub struct ColumnInfo {
    pub id: i64,
    pub offset: usize,
    pub primary_key: bool,
    pub unsigned: bool,
    pub needs_restored_data: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引元信息：列偏移列表与是否全局索引。
/// 索引元信息：列偏移列表与是否全局索引。
pub struct IndexInfo {
    pub columns: Vec<usize>,
    pub global: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 表元信息：列、整型句柄/公共句柄（common handle）与主键列 ID。
/// 表元信息：列、句柄形态（整型 PK / Common Handle）。
pub struct TableInfo {
    pub id: i64,
    pub columns: Vec<ColumnInfo>,
    pub pk_is_handle: bool,
    pub common_handle: bool,
    pub common_pk_column_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 内存读取路径错误分类。
/// MemReader 错误分类。
pub enum MemReaderError {
    Backend(String),
    Decode(String),
    Compare(String),
    Unsupported(String),
    Closed,
}

impl Display for MemReaderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for MemReaderError {}

/// 后端边界：快照读取、编解码、条件求值与句柄范围转换。
/// 后端：快照读取、键值解码、条件求值与 Handle↔Range 转换。
pub trait MemReaderBackend: Send + Sync + 'static {
    fn txn_snapshot(&self, range: &KeyRange, reverse: bool) -> Result<Vec<KvPair>, MemReaderError>;
    fn temporary_snapshot(
        &self,
        range: &KeyRange,
        reverse: bool,
    ) -> Result<Option<Vec<KvPair>>, MemReaderError>;
    fn cache_snapshot(
        &self,
        range: &KeyRange,
        reverse: bool,
    ) -> Result<Option<Vec<KvPair>>, MemReaderError>;
    fn decode_index_values(
        &self,
        key: &[u8],
        value: &[u8],
        index_columns: usize,
        types: &[FieldType],
    ) -> Result<Vec<Datum>, MemReaderError>;
    fn decode_row_handle(&self, key: &[u8]) -> Result<Handle, MemReaderError>;
    /// Decode one primary-key component from a common handle, matching Go's
    /// `Handle.EncodedCol(index)` behavior. Partition wrappers must be handled
    /// transparently by the backend.
    fn decode_common_handle_column(
        &self,
        handle: &Handle,
        index: usize,
    ) -> Result<Datum, MemReaderError>;
    fn decode_index_handle(
        &self,
        key: &[u8],
        value: &[u8],
        index_columns: usize,
    ) -> Result<Handle, MemReaderError>;
    fn decode_partition_id(&self, key: &[u8], value: &[u8]) -> Result<i64, MemReaderError>;
    fn decode_row(
        &self,
        table: &TableInfo,
        columns: &[ColumnInfo],
        handle: &Handle,
        value: &[u8],
    ) -> Result<BTreeMap<i64, Datum>, MemReaderError>;
    fn default_column_value(&self, column: &ColumnInfo) -> Result<Datum, MemReaderError>;
    fn evaluate_conditions(
        &self,
        conditions: &[String],
        row: &[Datum],
    ) -> Result<bool, MemReaderError>;
    fn compare_rows(&self, left: &[Datum], right: &[Datum]) -> Result<Ordering, MemReaderError>;
    fn table_handles_to_ranges(
        &self,
        table_id: i64,
        handles: &[Handle],
    ) -> Result<Vec<KeyRange>, MemReaderError>;
    fn int_handle_to_common(&self, handle: i64) -> Result<Vec<u8>, MemReaderError>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 结果比较/排序配置：降序与是否需要额外排序。
/// 排序比较选项：是否降序、是否需额外排序。
pub struct compareExec {
    pub desc: bool,
    pub needExtraSorting: bool,
}

#[derive(Clone)]
/// UnionScan 规格：表列、过滤条件、分区集合与比较配置。
/// UnionScan 规格：表/列/条件、分区集合与比较选项。
pub struct UnionScanSpec<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub conditions: Vec<String>,
    pub desc: bool,
    pub keep_order: bool,
    pub compare: compareExec,
    pub physical_table_id_index: Option<usize>,
    pub partition_ids: BTreeSet<i64>,
}

#[derive(Clone, Debug)]
/// 索引读取规格：索引定义、键范围与输出列偏移。
/// 索引读取规格：索引、键范围与输出列偏移。
pub struct IndexReaderSpec {
    pub index: IndexInfo,
    pub ranges: Vec<KeyRange>,
    pub output_offsets: Vec<usize>,
}

#[derive(Clone, Debug)]
/// 按物理表 ID 分组的键范围（分区表场景）。
/// 按物理表 ID 分组的键范围（分区场景）。
pub struct GroupedRanges {
    pub physical_table_id: i64,
    pub ranges: Vec<KeyRange>,
}

/// 可从内存缓冲提取行句柄集合的读取器。
/// 从内存缓冲提取 Handle 列表的公共接口。
pub trait memReader {
    /// 收集缓冲中满足范围的行句柄。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError>;
}

/// 索引侧内存读取器：扫描索引 KV 并解码为输出行。
/// 在事务内存缓冲上扫描索引项并解码为行。
pub struct memIndexReader<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub index: IndexInfo,
    pub table: TableInfo,
    pub kvRanges: Vec<KeyRange>,
    pub conditions: Vec<String>,
    pub addedRows: Vec<Row>,
    pub addedRowsLen: usize,
    pub retFieldTypes: Vec<FieldType>,
    pub outputOffset: Vec<usize>,
    pub keepOrder: bool,
    pub physTblIDIdx: Option<usize>,
    pub partitionIDMap: BTreeSet<i64>,
    pub compareExec: compareExec,
}

impl<B: MemReaderBackend> Clone for memIndexReader<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            index: self.index.clone(),
            table: self.table.clone(),
            kvRanges: self.kvRanges.clone(),
            conditions: self.conditions.clone(),
            addedRows: self.addedRows.clone(),
            addedRowsLen: self.addedRowsLen,
            retFieldTypes: self.retFieldTypes.clone(),
            outputOffset: self.outputOffset.clone(),
            keepOrder: self.keepOrder,
            physTblIDIdx: self.physTblIDIdx,
            partitionIDMap: self.partitionIDMap.clone(),
            compareExec: self.compareExec.clone(),
        }
    }
}

/// 由 UnionScan 与索引规格构造 `memIndexReader`；降序时反转范围。
/// 由 UnionScan 与 IndexReader 规格构造 memIndexReader。
pub fn buildMemIndexReader<B: MemReaderBackend>(
    us: &UnionScanSpec<B>,
    reader: &IndexReaderSpec,
) -> memIndexReader<B> {
    let mut ranges = reader.ranges.clone();
    if us.desc {
        ranges.reverse();
    }
    memIndexReader {
        backend: Arc::clone(&us.backend),
        index: reader.index.clone(),
        table: us.table.clone(),
        kvRanges: ranges,
        conditions: us.conditions.clone(),
        addedRows: Vec::new(),
        addedRowsLen: 0,
        retFieldTypes: us
            .columns
            .iter()
            .map(|column| FieldType {
                unsigned: column.unsigned,
            })
            .collect(),
        outputOffset: reader.output_offsets.clone(),
        keepOrder: us.keep_order,
        physTblIDIdx: us.physical_table_id_index,
        partitionIDMap: us.partition_ids.clone(),
        compareExec: us.compare.clone(),
    }
}

impl<B: MemReaderBackend> memIndexReader<B> {
    /// 返回行迭代器；若需保持顺序且额外排序，则先物化全部行。
    /// 若需额外排序则物化全部行；否则返回流式索引迭代器。
    pub fn getMemRowsIter(&mut self) -> Result<Box<dyn memRowsIter>, MemReaderError> {
        // 流式路径无法保证顺序时退化为全量排序
        if self.keepOrder && self.compareExec.needExtraSorting {
            return Ok(Box::new(defaultRowsIter {
                data: self.getMemRows()?,
                cursor: 0,
            }));
        }
        Ok(Box::new(memRowsIterForIndex {
            kvIter: newTxnMemBufferIter(
                Arc::clone(&self.backend),
                self.kvRanges.clone(),
                self.compareExec.desc,
            )?,
            memIndexReader: self.clone(),
        }))
    }

    /// 推导索引解码所需的字段类型序列（含句柄列）。
    /// 索引列 + 句柄列的 FieldType 列表，供解码使用。
    pub fn getTypes(&self) -> Vec<FieldType> {
        let mut types = self
            .index
            .columns
            .iter()
            .map(|offset| FieldType {
                unsigned: self.table.columns[*offset].unsigned,
            })
            .collect::<Vec<_>>();
        if self.table.pk_is_handle {
            if let Some(column) = self.table.columns.iter().find(|column| column.primary_key) {
                types.push(FieldType {
                    unsigned: column.unsigned,
                });
            }
        } else if self.table.common_handle {
            for id in &self.table.common_pk_column_ids {
                if let Some(column) = self.table.columns.iter().find(|column| column.id == *id) {
                    types.push(FieldType {
                        unsigned: column.unsigned,
                    });
                }
            }
        } else {
            types.push(FieldType { unsigned: false });
        }
        types
    }

    /// 物化索引缓冲中的全部匹配行；必要时按比较配置排序。
    /// 遍历索引 KV：全局索引做分区过滤，求值条件后可选排序。
    pub fn getMemRows(&mut self) -> Result<Vec<Row>, MemReaderError> {
        self.addedRows.clear();
        // 解码类型依赖索引列与句柄形态
        let types = self.getTypes();
        let mut rows = Vec::new();
        iterTxnMemBuffer(
            Arc::clone(&self.backend),
            self.kvRanges.clone(),
            self.compareExec.desc,
            &mut |key, value| {
                // 全局索引需按分区 ID 过滤
                // 全局索引：跳过不属于当前分区集合的项
                if self.index.global
                    && !self
                        .partitionIDMap
                        .contains(&self.backend.decode_partition_id(key, value)?)
                {
                    return Ok(());
                }
                let row = self.decodeIndexKeyValue(key, value, &types)?;
                if self.backend.evaluate_conditions(&self.conditions, &row)? {
                    rows.push(row);
                }
                Ok(())
            },
        )?;
        if self.keepOrder && self.compareExec.needExtraSorting {
            sort_rows(self.backend.as_ref(), &mut rows, self.compareExec.desc)?;
        }
        self.addedRows = rows.clone();
        Ok(rows)
    }

    /// 将索引 KV 解码为输出行，并在需要时填充分区物理表 ID。
    /// 解码索引 KV，并按 outputOffset 组装结果行（含物理表 ID 列）。
    pub fn decodeIndexKeyValue(
        &self,
        key: &[u8],
        value: &[u8],
        types: &[FieldType],
    ) -> Result<Row, MemReaderError> {
        let decoded =
            self.backend
                .decode_index_values(key, value, self.index.columns.len(), types)?;
        // 输出偏移含物理表 ID 列时，从键中解码分区 ID 填入
        let physical_offset = self
            .physTblIDIdx
            .and_then(|index| self.outputOffset.get(index).copied());
        let mut result = Vec::with_capacity(self.outputOffset.len());
        for (index, output_offset) in self.outputOffset.iter().copied().enumerate() {
            if self.physTblIDIdx == Some(index) {
                result.push(Datum::Signed(self.backend.decode_partition_id(key, value)?));
                continue;
            }
            let adjusted = if physical_offset.is_some_and(|offset| output_offset > offset) {
                output_offset - 1
            } else {
                output_offset
            };
            result.push(decoded.get(adjusted).cloned().ok_or_else(|| {
                MemReaderError::Decode(format!("index output offset {adjusted} is out of range"))
            })?);
        }
        Ok(result)
    }
}

impl<B: MemReaderBackend> memReader for memIndexReader<B> {
    /// 从索引 KV 解码 Handle，Common Handle 时转换整型内层；过滤分区。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        let mut handles = Vec::with_capacity(self.addedRowsLen);
        iterTxnMemBuffer(
            Arc::clone(&self.backend),
            self.kvRanges.clone(),
            self.compareExec.desc,
            &mut |key, value| {
                let mut handle =
                    self.backend
                        .decode_index_handle(key, value, self.index.columns.len())?;
                // 公共句柄表：整型句柄需转为 Common 字节形式
                // Common Handle：把整型句柄转为字节形式
                if self.table.common_handle {
                    match handle {
                        Handle::Int(value) => {
                            handle = Handle::Common(self.backend.int_handle_to_common(value)?)
                        }
                        Handle::Partition {
                            partition_id,
                            ref inner,
                        } if matches!(**inner, Handle::Int(_)) => {
                            let Handle::Int(value) = **inner else {
                                unreachable!()
                            };
                            handle = Handle::Partition {
                                partition_id,
                                inner: Box::new(Handle::Common(
                                    self.backend.int_handle_to_common(value)?,
                                )),
                            };
                        }
                        _ => {}
                    }
                }
                if let Handle::Partition { partition_id, .. } = &handle {
                    if !self.partitionIDMap.contains(partition_id) {
                        return Ok(());
                    }
                }
                handles.push(handle);
                Ok(())
            },
        )?;
        Ok(handles)
    }
}

#[derive(Clone, Debug, Default)]
/// 解码过程中复用的句柄字节与列值缓冲。
/// 行解码可复用缓冲。
pub struct allocBuf {
    pub handleBytes: Vec<u8>,
    pub decoded: BTreeMap<i64, Datum>,
}

/// 表侧内存读取器：按记录键解码整行。
/// 在事务内存缓冲上按表键范围解码记录行。
pub struct memTableReader<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub kvRanges: Vec<KeyRange>,
    pub conditions: Vec<String>,
    pub addedRows: Vec<Row>,
    pub retFieldTypes: Vec<FieldType>,
    pub colIDs: BTreeMap<i64, usize>,
    pub buffer: allocBuf,
    pub pkColIDs: Vec<i64>,
    pub offsets: Vec<usize>,
    pub keepOrder: bool,
    pub compareExec: compareExec,
}

impl<B: MemReaderBackend> Clone for memTableReader<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            table: self.table.clone(),
            columns: self.columns.clone(),
            kvRanges: self.kvRanges.clone(),
            conditions: self.conditions.clone(),
            addedRows: self.addedRows.clone(),
            retFieldTypes: self.retFieldTypes.clone(),
            colIDs: self.colIDs.clone(),
            buffer: self.buffer.clone(),
            pkColIDs: self.pkColIDs.clone(),
            offsets: self.offsets.clone(),
            keepOrder: self.keepOrder,
            compareExec: self.compareExec.clone(),
        }
    }
}

/// 由 UnionScan 与键范围构造 `memTableReader`。
/// 由 UnionScan 规格与键范围构造 memTableReader。
pub fn buildMemTableReader<B: MemReaderBackend>(
    us: &UnionScanSpec<B>,
    mut ranges: Vec<KeyRange>,
) -> memTableReader<B> {
    let (column_ids, primary_ids, buffer) =
        getColIDAndPkColIDs(us.backend.as_ref(), &us.table, &us.columns);
    if us.desc {
        ranges.reverse();
    }
    memTableReader {
        backend: Arc::clone(&us.backend),
        table: us.table.clone(),
        columns: us.columns.clone(),
        kvRanges: ranges,
        conditions: us.conditions.clone(),
        addedRows: Vec::new(),
        retFieldTypes: us
            .columns
            .iter()
            .map(|c| FieldType {
                unsigned: c.unsigned,
            })
            .collect(),
        colIDs: column_ids,
        buffer,
        pkColIDs: primary_ids,
        offsets: Vec::new(),
        keepOrder: us.keep_order,
        compareExec: us.compare.clone(),
    }
}

impl<B: MemReaderBackend> memTableReader<B> {
    /// 返回表行迭代器；保持顺序且需额外排序时先物化。
    /// 若需额外排序则物化全部行；否则返回流式表迭代器。
    pub fn getMemRowsIter(&mut self) -> Result<Box<dyn memRowsIter>, MemReaderError> {
        if self.keepOrder && self.compareExec.needExtraSorting {
            return Ok(Box::new(defaultRowsIter {
                data: self.getMemRows()?,
                cursor: 0,
            }));
        }
        self.offsets = self
            .columns
            .iter()
            .map(|column| self.colIDs[&column.id])
            .collect();
        Ok(Box::new(memRowsIterForTable {
            kvIter: newTxnMemBufferIter(
                Arc::clone(&self.backend),
                self.kvRanges.clone(),
                self.compareExec.desc,
            )?,
            memTableReader: self.clone(),
        }))
    }

    /// 物化表缓冲中的全部匹配行。
    /// 遍历表记录 KV：解码行、条件过滤后可选排序。
    pub fn getMemRows(&mut self) -> Result<Vec<Row>, MemReaderError> {
        self.offsets = self
            .columns
            .iter()
            .map(|column| self.colIDs[&column.id])
            .collect();
        let mut rows = Vec::new();
        iterTxnMemBuffer(
            Arc::clone(&self.backend),
            self.kvRanges.clone(),
            self.compareExec.desc,
            &mut |key, value| {
                let row = self.decodeRecordKeyValue(key, value)?;
                if self.backend.evaluate_conditions(&self.conditions, &row)? {
                    rows.push(row);
                }
                Ok(())
            },
        )?;
        if self.keepOrder && self.compareExec.needExtraSorting {
            sort_rows(self.backend.as_ref(), &mut rows, self.compareExec.desc)?;
        }
        self.addedRows = rows.clone();
        Ok(rows)
    }

    /// 从记录键值解码出行句柄再取列数据。
    /// 从记录键解码 Handle 再解出完整行。
    pub fn decodeRecordKeyValue(&self, key: &[u8], value: &[u8]) -> Result<Row, MemReaderError> {
        let handle = self.backend.decode_row_handle(key)?;
        self.decodeRowData(&handle, value)
    }

    /// 按投影列顺序组装一行。
    /// 按投影列顺序取出已解码列值。
    pub fn decodeRowData(&self, handle: &Handle, value: &[u8]) -> Result<Row, MemReaderError> {
        let values = self.getRowData(handle, value)?;
        self.columns
            .iter()
            .map(|column| {
                values.get(&column.id).cloned().ok_or_else(|| {
                    MemReaderError::Decode(format!("column {} is absent", column.id))
                })
            })
            .collect()
    }

    /// 解码行内容并补齐主键/默认值缺失列。
    /// 解码行并补齐缺省列：主键句柄列或默认值。
    pub fn getRowData(
        &self,
        handle: &Handle,
        value: &[u8],
    ) -> Result<BTreeMap<i64, Datum>, MemReaderError> {
        let mut values = self
            .backend
            .decode_row(&self.table, &self.columns, handle, value)?;
        for column in &self.columns {
            // 已解码列跳过；否则从句柄或默认值补齐
            if values.contains_key(&column.id) {
                continue;
            }
            // 公共主键列且无需还原数据时，直接用 Handle 填值
            if self.table.common_handle
                && self.pkColIDs.contains(&column.id)
                && !column.needs_restored_data
            {
                let index = self
                    .pkColIDs
                    .iter()
                    .position(|id| *id == column.id)
                    .expect("common primary-key column was checked above");
                values.insert(
                    column.id,
                    self.backend.decode_common_handle_column(handle, index)?,
                );
            } else if (self.table.pk_is_handle && column.primary_key) || column.id == -1 {
                values.insert(
                    column.id,
                    match handle {
                        Handle::Int(value) if column.unsigned => Datum::Unsigned(*value as u64),
                        Handle::Int(value) => Datum::Signed(*value),
                        _ => handle_datum(handle),
                    },
                );
            } else {
                values.insert(column.id, self.backend.default_column_value(column)?);
            }
        }
        Ok(values)
    }
}

impl<B: MemReaderBackend> memReader for memTableReader<B> {
    /// 从表记录键解码全部 Handle。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        let mut handles = Vec::new();
        iterTxnMemBuffer(
            Arc::clone(&self.backend),
            self.kvRanges.clone(),
            self.compareExec.desc,
            &mut |key, _| {
                handles.push(self.backend.decode_row_handle(key)?);
                Ok(())
            },
        )?;
        Ok(handles)
    }
}

/// 判断给定列 ID 在稀疏列数组中是否已有非空值。
/// 判断列 ID 在稀疏行缓冲中是否已有值。
pub fn hasColVal(data: &[Option<Value>], column_ids: &BTreeMap<i64, usize>, id: i64) -> bool {
    column_ids
        .get(&id)
        .and_then(|offset| data.get(*offset))
        .is_some_and(Option::is_some)
}

/// 事务写缓冲与快照合并后的 KV 迭代器。
/// 跨多段 KeyRange 的事务内存缓冲迭代器。
pub struct txnMemBufferIter<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub kvRanges: Vec<KeyRange>,
    pub idx: usize,
    pub curr: Vec<KvPair>,
    pub cursor: usize,
    pub reverse: bool,
    pub err: Option<MemReaderError>,
    pub closed: bool,
}

/// 构造跨多个键范围的缓冲迭代器。
/// 构造尚未加载任何 range 的迭代器。
pub fn newTxnMemBufferIter<B: MemReaderBackend>(
    backend: Arc<B>,
    ranges: Vec<KeyRange>,
    reverse: bool,
) -> Result<txnMemBufferIter<B>, MemReaderError> {
    Ok(txnMemBufferIter {
        backend,
        kvRanges: ranges,
        idx: 0,
        curr: Vec::new(),
        cursor: 0,
        reverse,
        err: None,
        closed: false,
    })
}

impl<B: MemReaderBackend> txnMemBufferIter<B> {
    // 加载下一个非空合并范围
    /// 合并下一 KeyRange 的快照与事务写，跳过空段。
    fn load_next_range(&mut self) -> Result<bool, MemReaderError> {
        while self.idx < self.kvRanges.len() {
            self.curr = union_range(
                self.backend.as_ref(),
                &self.kvRanges[self.idx],
                self.reverse,
            )?;
            self.idx += 1;
            self.cursor = 0;
            if !self.curr.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// 当前位置是否有效；必要时推进到下一范围。
    /// 当前位置是否有效；必要时加载下一段。
    pub fn Valid(&mut self) -> bool {
        if self.closed {
            return false;
        }
        if self.cursor < self.curr.len() {
            return true;
        }
        match self.load_next_range() {
            Ok(valid) => valid,
            Err(error) => {
                self.err = Some(error);
                true
            }
        }
    }
    /// 前进到下一对 KV；若上次加载出错则在此抛出。
    /// 前进一格；若上次加载出错则在此返回。
    pub fn Next(&mut self) -> Result<(), MemReaderError> {
        if let Some(error) = self.err.take() {
            return Err(error);
        }
        if self.cursor < self.curr.len() {
            self.cursor += 1;
        }
        Ok(())
    }
    /// 当前键。
    /// 当前键。
    pub fn Key(&self) -> &[u8] {
        &self.curr[self.cursor].key
    }
    /// 当前值。
    /// 当前值。
    pub fn Value(&self) -> &[u8] {
        &self.curr[self.cursor].value
    }
    /// 关闭迭代器并清空当前缓冲。
    /// 关闭并释放当前段缓冲。
    pub fn Close(&mut self) {
        self.closed = true;
        self.curr.clear();
    }
}

/// 遍历合并后的缓冲 KV，跳过空值（删除），并对每对调用回调。
/// 遍历内存缓冲：跳过空值（删除标记），对其余 KV 调用回调。
pub fn iterTxnMemBuffer<B: MemReaderBackend>(
    backend: Arc<B>,
    ranges: Vec<KeyRange>,
    reverse: bool,
    function: &mut processKVFunc<'_>,
) -> Result<(), MemReaderError> {
    let mut iterator = newTxnMemBufferIter(backend, ranges, reverse)?;
    while iterator.Valid() {
        let key = iterator.Key().to_vec();
        let value = iterator.Value().to_vec();
        iterator.Next()?;
        if value.is_empty() {
            continue;
        }
        function(&key, &value)?;
    }
    iterator.Close();
    Ok(())
}

/// 优先取临时快照，否则回退到缓存快照。
/// 优先临时表快照，否则缓存快照。
pub fn getSnapIter<B: MemReaderBackend>(
    backend: &B,
    range: &KeyRange,
    reverse: bool,
) -> Result<Option<Vec<KvPair>>, MemReaderError> {
    match backend.temporary_snapshot(range, reverse)? {
        Some(iterator) => Ok(Some(iterator)),
        None => backend.cache_snapshot(range, reverse),
    }
}

/// 将快照与事务缓冲按 key 合并；同 key 以事务侧覆盖。
/// 合并快照与事务写缓冲：同键以事务侧覆盖，并可按需反转。
fn union_range<B: MemReaderBackend>(
    backend: &B,
    range: &KeyRange,
    reverse: bool,
) -> Result<Vec<KvPair>, MemReaderError> {
    // 先放入快照，再以事务缓冲覆盖同 key
    let mut merged = BTreeMap::<Key, Value>::new();
    if let Some(snapshot) = getSnapIter(backend, range, reverse)? {
        for pair in snapshot {
            merged.insert(pair.key, pair.value);
        }
    }
    // 事务写覆盖同键的快照值
    for pair in backend.txn_snapshot(range, reverse)? {
        merged.insert(pair.key, pair.value);
    }
    let mut result = merged
        .into_iter()
        .map(|(key, value)| KvPair { key, value })
        .collect::<Vec<_>>();
    if reverse {
        result.reverse();
    }
    Ok(result)
}

#[derive(Clone)]
/// IndexLookUp：先从索引取句柄，再回表读取完整行。
/// IndexLookUp：先取索引 Handle，再回表读行。
pub struct memIndexLookUpReader<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub conditions: Vec<String>,
    pub idxReader: memIndexReader<B>,
    pub groupedKVRanges: Vec<GroupedRanges>,
    pub keepOrder: bool,
    pub compareExec: compareExec,
}

/// 构造 IndexLookUp 内存读取器；索引输出仅保留句柄列偏移。
/// 构造 IndexLookUp 内存读取器；输出偏移指向句柄列。
pub fn buildMemIndexLookUpReader<B: MemReaderBackend>(
    us: &UnionScanSpec<B>,
    reader: &IndexReaderSpec,
    grouped: Vec<GroupedRanges>,
) -> memIndexLookUpReader<B> {
    let mut index_reader = buildMemIndexReader(us, reader);
    index_reader.outputOffset = vec![reader.index.columns.len()];
    memIndexLookUpReader {
        backend: Arc::clone(&us.backend),
        table: us.table.clone(),
        columns: us.columns.clone(),
        conditions: us.conditions.clone(),
        idxReader: index_reader,
        groupedKVRanges: grouped,
        keepOrder: us.keep_order,
        compareExec: us.compare.clone(),
    }
}

impl<B: MemReaderBackend> memIndexLookUpReader<B> {
    /// 按分组范围收集索引句柄，转换为表范围后委托表读取器。
    // 无分组时退化为整表索引范围
    /// 由索引 Handle 转表 Range，再委托 memTableReader 迭代。
    pub fn getMemRowsIter(&mut self) -> Result<Box<dyn memRowsIter>, MemReaderError> {
        let groups = if self.groupedKVRanges.is_empty() {
            vec![GroupedRanges {
                physical_table_id: self.table.id,
                ranges: self.idxReader.kvRanges.clone(),
            }]
        } else {
            self.groupedKVRanges.clone()
        };
        // 索引句柄 → 表记录键范围
        let mut table_ranges = Vec::new();
        let mut count = 0;
        for group in groups {
            self.idxReader.kvRanges = group.ranges;
            let handles = self.idxReader.getMemRowsHandle()?;
            count += handles.len();
            table_ranges.extend(
                self.backend
                    .table_handles_to_ranges(group.physical_table_id, &handles)?,
            );
        }
        if count == 0 {
            return Ok(Box::new(defaultRowsIter::default()));
        }
        if self.compareExec.desc {
            table_ranges.reverse();
        }
        let (col_ids, pk_ids, buffer) =
            getColIDAndPkColIDs(self.backend.as_ref(), &self.table, &self.columns);
        let mut table_reader = memTableReader {
            backend: Arc::clone(&self.backend),
            table: self.table.clone(),
            columns: self.columns.clone(),
            kvRanges: table_ranges,
            conditions: self.conditions.clone(),
            addedRows: Vec::with_capacity(count),
            retFieldTypes: self
                .columns
                .iter()
                .map(|c| FieldType {
                    unsigned: c.unsigned,
                })
                .collect(),
            colIDs: col_ids,
            buffer,
            pkColIDs: pk_ids,
            offsets: Vec::new(),
            keepOrder: self.keepOrder,
            compareExec: self.compareExec.clone(),
        };
        table_reader.getMemRowsIter()
    }
}

impl<B: MemReaderBackend> memReader for memIndexLookUpReader<B> {
    /// IndexLookUp 不支持直接取 Handle。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        Err(MemReaderError::Unsupported(
            "getMemRowsHandle for memIndexLookUpReader".into(),
        ))
    }
}

/// IndexMerge 的局部读取器：表或索引二选一。
/// IndexMerge 的局部读取器：表扫或索引扫。
pub enum PartialMemReader<B: MemReaderBackend> {
    Table(memTableReader<B>),
    Index(memIndexReader<B>),
}
impl<B: MemReaderBackend> PartialMemReader<B> {
    /// 更新局部读取器的扫描范围。
    /// 更新局部读取器的键范围。
    fn set_ranges(&mut self, ranges: Vec<KeyRange>) {
        match self {
            Self::Table(reader) => reader.kvRanges = ranges,
            Self::Index(reader) => reader.kvRanges = ranges,
        }
    }
}
impl<B: MemReaderBackend> memReader for PartialMemReader<B> {
    /// 转发到表或索引局部读取器。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        match self {
            Self::Table(reader) => reader.getMemRowsHandle(),
            Self::Index(reader) => reader.getMemRowsHandle(),
        }
    }
}

/// IndexMerge：多路局部扫描后按交/并集合并句柄再回表。
/// IndexMerge：多路 Handle 求交/并后回表。
pub struct memIndexMergeReader<B: MemReaderBackend> {
    pub backend: Arc<B>,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub conditions: Vec<String>,
    pub memReaders: Vec<PartialMemReader<B>>,
    pub isIntersection: bool,
    pub partitionMode: bool,
    pub partialWorkerKVRanges: Vec<Vec<GroupedRanges>>,
    pub keepOrder: bool,
    pub compareExec: compareExec,
}

/// 构造 IndexMerge；`None` 局部规格表示表扫描分支。
/// 由可选索引规格列表构造 IndexMerge 内存读取器。
pub fn buildMemIndexMergeReader<B: MemReaderBackend>(
    us: &UnionScanSpec<B>,
    readers: Vec<Option<IndexReaderSpec>>,
    ranges: Vec<Vec<GroupedRanges>>,
    intersection: bool,
    partition_mode: bool,
) -> memIndexMergeReader<B> {
    let mut partials = Vec::with_capacity(readers.len());
    for reader in readers {
        match reader {
            Some(index) => {
                let mut r = buildMemIndexReader(us, &index);
                r.outputOffset = vec![index.index.columns.len()];
                partials.push(PartialMemReader::Index(r));
            }
            None => partials.push(PartialMemReader::Table(buildMemTableReader(us, Vec::new()))),
        }
    }
    memIndexMergeReader {
        backend: Arc::clone(&us.backend),
        table: us.table.clone(),
        columns: us.columns.clone(),
        conditions: us.conditions.clone(),
        memReaders: partials,
        isIntersection: intersection,
        partitionMode: partition_mode,
        partialWorkerKVRanges: ranges,
        keepOrder: us.keep_order,
        compareExec: us.compare.clone(),
    }
}

impl<B: MemReaderBackend> memIndexMergeReader<B> {
    /// 物化合并结果后包装为默认行迭代器。
    /// 物化全部合并结果行后以 defaultRowsIter 返回。
    pub fn getMemRowsIter(&mut self) -> Result<Box<dyn memRowsIter>, MemReaderError> {
        Ok(Box::new(defaultRowsIter {
            data: self.getMemRows()?,
            cursor: 0,
        }))
    }
    /// 汇总各局部读取器句柄；交集模式要求出现在全部局部结果中。
    // 分区模式将普通句柄包装为带物理表 ID 的 Partition 句柄
    /// 收集各局部读取器 Handle；交集模式要求出现在全部路径。
    pub fn getHandles(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        // 统计句柄在各局部路径出现次数，用于交集过滤
        let mut counts = BTreeMap::<Handle, usize>::new();
        for (reader_index, reader) in self.memReaders.iter_mut().enumerate() {
            for group in self
                .partialWorkerKVRanges
                .get(reader_index)
                .cloned()
                .unwrap_or_default()
            {
                reader.set_ranges(group.ranges);
                for mut handle in reader.getMemRowsHandle()? {
                    if self.partitionMode && !matches!(handle, Handle::Partition { .. }) {
                        handle = Handle::Partition {
                            partition_id: group.physical_table_id,
                            inner: Box::new(handle),
                        };
                    }
                    *counts.entry(handle).or_default() += 1;
                }
            }
        }
        Ok(counts
            .into_iter()
            .filter_map(|(handle, count)| {
                // 并集：保留出现过的 Handle；交集：须在所有局部路径出现
                if !self.isIntersection || count == self.memReaders.len() {
                    Some(handle)
                } else {
                    None
                }
            })
            .collect())
    }
    /// 由合并句柄回表取行，并按需排序。
    /// 合并 Handle 回表读行，必要时按 keepOrder 排序。
    pub fn getMemRows(&mut self) -> Result<Vec<Row>, MemReaderError> {
        let handles = self.getHandles()?;
        if handles.is_empty() {
            return Ok(Vec::new());
        }
        let table_id = if self.partitionMode { 0 } else { self.table.id };
        let ranges = self.backend.table_handles_to_ranges(table_id, &handles)?;
        let (col_ids, pk_ids, buffer) =
            getColIDAndPkColIDs(self.backend.as_ref(), &self.table, &self.columns);
        let mut reader = memTableReader {
            backend: Arc::clone(&self.backend),
            table: self.table.clone(),
            columns: self.columns.clone(),
            kvRanges: ranges,
            conditions: self.conditions.clone(),
            addedRows: Vec::with_capacity(handles.len()),
            retFieldTypes: self
                .columns
                .iter()
                .map(|c| FieldType {
                    unsigned: c.unsigned,
                })
                .collect(),
            colIDs: col_ids,
            buffer,
            pkColIDs: pk_ids,
            offsets: Vec::new(),
            keepOrder: false,
            compareExec: compareExec::default(),
        };
        let mut rows = reader.getMemRows()?;
        if self.keepOrder {
            sort_rows(self.backend.as_ref(), &mut rows, self.compareExec.desc)?;
        }
        Ok(rows)
    }
}

impl<B: MemReaderBackend> memReader for memIndexMergeReader<B> {
    /// IndexMerge 不支持直接取 Handle。
    fn getMemRowsHandle(&mut self) -> Result<Vec<Handle>, MemReaderError> {
        Err(MemReaderError::Unsupported(
            "getMemRowsHandle for memIndexMergeReader".into(),
        ))
    }
}

/// 逐行拉取的内存结果迭代器。
/// 行迭代器：Next 取下一行，Close 结束。
pub trait memRowsIter {
    /// 取下一行；耗尽返回 `None`。
    fn Next(&mut self) -> Result<Option<Row>, MemReaderError>;
    /// 释放迭代资源。
    fn Close(&mut self);
}

#[derive(Default)]
/// 基于已物化行向量的简单游标迭代器。
/// 基于已物化 Vec 的简单行迭代器。
pub struct defaultRowsIter {
    pub data: Vec<Row>,
    pub cursor: usize,
}
impl memRowsIter for defaultRowsIter {
    fn Next(&mut self) -> Result<Option<Row>, MemReaderError> {
        if self.cursor >= self.data.len() {
            Ok(None)
        } else {
            let row = self.data[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        }
    }
    fn Close(&mut self) {
        self.cursor = self.data.len();
    }
}

/// 表侧流式迭代：边扫缓冲边解码过滤。
/// 流式表行迭代：边扫内存缓冲边解码/过滤。
pub struct memRowsIterForTable<B: MemReaderBackend> {
    pub kvIter: txnMemBufferIter<B>,
    pub memTableReader: memTableReader<B>,
}
impl<B: MemReaderBackend> memRowsIter for memRowsIterForTable<B> {
    fn Next(&mut self) -> Result<Option<Row>, MemReaderError> {
        while self.kvIter.Valid() {
            let key = self.kvIter.Key().to_vec();
            let value = self.kvIter.Value().to_vec();
            self.kvIter.Next()?;
            if value.is_empty() {
                continue;
            }
            let row = self.memTableReader.decodeRecordKeyValue(&key, &value)?;
            if self
                .memTableReader
                .backend
                .evaluate_conditions(&self.memTableReader.conditions, &row)?
            {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
    fn Close(&mut self) {
        self.kvIter.Close();
    }
}

/// 索引侧流式迭代：过滤全局索引分区并解码输出。
/// 流式索引行迭代：含全局索引分区过滤。
pub struct memRowsIterForIndex<B: MemReaderBackend> {
    pub kvIter: txnMemBufferIter<B>,
    pub memIndexReader: memIndexReader<B>,
}
impl<B: MemReaderBackend> memRowsIter for memRowsIterForIndex<B> {
    fn Next(&mut self) -> Result<Option<Row>, MemReaderError> {
        let types = self.memIndexReader.getTypes();
        while self.kvIter.Valid() {
            let key = self.kvIter.Key().to_vec();
            let value = self.kvIter.Value().to_vec();
            self.kvIter.Next()?;
            if value.is_empty() {
                continue;
            }
            if self.memIndexReader.index.global
                && !self.memIndexReader.partitionIDMap.contains(
                    &self
                        .memIndexReader
                        .backend
                        .decode_partition_id(&key, &value)?,
                )
            {
                continue;
            }
            let row = self
                .memIndexReader
                .decodeIndexKeyValue(&key, &value, &types)?;
            if self
                .memIndexReader
                .backend
                .evaluate_conditions(&self.memIndexReader.conditions, &row)?
            {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
    fn Close(&mut self) {
        self.kvIter.Close();
    }
}

/// 构建列 ID→偏移映射、主键列 ID 列表与可复用解码缓冲。
/// 建立列 ID→偏移映射，并准备主键列 ID 与解码缓冲。
pub fn getColIDAndPkColIDs<B: MemReaderBackend>(
    _backend: &B,
    table: &TableInfo,
    columns: &[ColumnInfo],
) -> (BTreeMap<i64, usize>, Vec<i64>, allocBuf) {
    let ids = columns
        .iter()
        .enumerate()
        .map(|(index, column)| (column.id, index))
        .collect();
    let primary = if table.common_pk_column_ids.is_empty() {
        vec![-1]
    } else {
        table.common_pk_column_ids.clone()
    };
    (
        ids,
        primary,
        allocBuf {
            handleBytes: Vec::with_capacity(16),
            decoded: BTreeMap::new(),
        },
    )
}

/// 将句柄转为 Datum；分区句柄递归取内层。
/// 将 Handle 转为 Datum（分区句柄递归取内层）。
fn handle_datum(handle: &Handle) -> Datum {
    match handle {
        Handle::Int(value) => Datum::Signed(*value),
        Handle::Common(value) => Datum::Bytes(value.clone()),
        Handle::Partition { inner, .. } => handle_datum(inner),
    }
}

/// 按后端行比较器排序；降序时反转序关系。
/// 按 backend.compare_rows 对行排序；比较失败时记下错误。
fn sort_rows<B: MemReaderBackend>(
    backend: &B,
    rows: &mut [Row],
    descending: bool,
) -> Result<(), MemReaderError> {
    let mut error = None;
    rows.sort_by(|left, right| match backend.compare_rows(left, right) {
        Ok(ordering) => {
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
        Err(cause) => {
            error = Some(cause);
            Ordering::Equal
        }
    });
    error.map_or(Ok(()), Err)
}
