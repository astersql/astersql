// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// ADMIN 维护类执行器：检查/恢复/清理索引。
//
// 对应 `ADMIN CHECK INDEX`、`ADMIN RECOVER INDEX`、`ADMIN CLEANUP INDEX` 等语句。
// 通过下推 DAG 扫描（IndexScan/TableScan）核对索引与表数据一致性，
// 并在事务中回填缺失索引项或删除悬空（dangling）索引项。
// Handle 为行标识（整数 _tidb_rowid 或聚簇索引编码键）。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::HashMap;
use std::sync::Arc;

use astersql_errors as errors;

/// ADMIN 执行路径统一结果类型。
pub type AdapterResult<T = ()> = Result<T, errors::SharedError>;
/// 编码后的 TiKV 键。
pub type Key = Vec<u8>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 单元格取值（空/整型/字节/文本），用于索引列与 handle。
pub enum Datum {
    Null,
    Int(i64),
    Uint(u64),
    Bytes(Vec<u8>),
    Text(String),
}

impl Datum {
    /// 将整型 Datum 转为 i64（无符号溢出则报错）。
    fn as_i64(&self) -> AdapterResult<i64> {
        match self {
            Self::Int(value) => Ok(*value),
            Self::Uint(value) => i64::try_from(*value)
                .map_err(|_| errors::New("unsigned handle does not fit into i64")),
            _ => Err(errors::New("handle column is not an integer")),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一行：按列偏移存放 Datum。
pub struct Row {
    pub values: Vec<Datum>,
}

impl Row {
    /// 按列偏移取 Datum，越界报错。
    fn datum(&self, offset: usize) -> AdapterResult<&Datum> {
        self.values
            .get(offset)
            .ok_or_else(|| errors::New(format!("row column {offset} is out of bounds")))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ADMIN 路径使用的简易行批（chunk）。
pub struct AdminChunk {
    pub rows: Vec<Row>,
}

impl AdminChunk {
    /// 清空已有行。
    pub fn Reset(&mut self) {
        self.rows.clear();
    }

    /// 当前行数。
    pub fn NumRows(&self) -> usize {
        self.rows.len()
    }

    /// 追加多行。
    pub fn AppendRows(&mut self, rows: impl IntoIterator<Item = Row>) {
        self.rows.extend(rows);
    }

    /// 向当前（或新建）行的指定列写入 i64。
    pub fn AppendInt64(&mut self, column: usize, value: i64) {
        self.append_value(column, Datum::Int(value));
    }

    /// 向当前（或新建）行的指定列写入 u64。
    pub fn AppendUint64(&mut self, column: usize, value: u64) {
        self.append_value(column, Datum::Uint(value));
    }

    /// 按列写入 Datum；列 0 或空批时新建行。
    fn append_value(&mut self, column: usize, value: Datum) {
        if column == 0 || self.rows.is_empty() {
            self.rows.push(Row::default());
        }
        let row = self.rows.last_mut().expect("row was inserted above");
        if row.values.len() <= column {
            row.values.resize(column + 1, Datum::Null);
        }
        row.values[column] = value;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 字段类型（含可选数组元素类型）。
pub struct FieldType {
    pub name: String,
    pub array_element: Option<Box<FieldType>>,
}

impl FieldType {
    /// 返回数组元素类型，若无则克隆自身。
    fn ArrayType(&self) -> Self {
        self.array_element
            .as_deref()
            .cloned()
            .unwrap_or_else(|| self.clone())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表列元信息。
pub struct ColumnInfo {
    pub ID: i64,
    pub Offset: usize,
    pub Name: String,
    pub FieldType: FieldType,
    pub Generated: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引中的列：记录在表中的 Offset。
pub struct IndexColumn {
    pub Offset: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引元信息（全局/主键/条件索引等）。
pub struct IndexInfo {
    pub ID: i64,
    pub Name: String,
    pub Columns: Vec<IndexColumn>,
    pub Global: bool,
    pub Primary: bool,
    pub HasCondition: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区定义：物理表 ID。
pub struct PartitionDefinition {
    pub ID: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表元信息：列、主键形态与分区列表。
pub struct TableInfo {
    pub ID: i64,
    pub Name: String,
    pub Columns: Vec<ColumnInfo>,
    pub PKIsHandle: bool,
    pub IsCommonHandle: bool,
    pub PrimaryColumnIds: Vec<i64>,
    pub Partitions: Vec<PartitionDefinition>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Handle 整数闭开区间 [Begin, End)。
pub struct HandleRange {
    pub Begin: i64,
    pub End: i64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 行 Handle：编码键、可选整数形式与分区 ID。
pub struct Handle {
    pub encoded: Vec<u8>,
    pub integer: Option<i64>,
    pub partition_id: Option<i64>,
}

impl Handle {
    /// 由整数行号构造 Handle。
    pub fn integer(value: i64) -> Self {
        Self {
            encoded: value.to_be_bytes().to_vec(),
            integer: Some(value),
            partition_id: None,
        }
    }

    /// 为分区表拼接 partition_id 前缀。
    pub fn partitioned(partition_id: i64, handle: &Self) -> Self {
        let mut encoded = partition_id.to_be_bytes().to_vec();
        encoded.extend_from_slice(&handle.encoded);
        Self {
            encoded,
            integer: handle.integer,
            partition_id: Some(partition_id),
        }
    }

    /// 字典序下一个 Handle（用于范围扫描续扫）。
    fn successor(&self) -> Option<Self> {
        if let Some(value) = self.integer {
            return value.checked_add(1).map(Self::integer);
        }
        let mut encoded = self.encoded.clone();
        for byte in encoded.iter_mut().rev() {
            if *byte != u8::MAX {
                *byte += 1;
                return Some(Self {
                    encoded,
                    integer: None,
                    partition_id: self.partition_id,
                });
            }
            *byte = 0;
        }
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 键范围 [StartKey, EndKey)。
pub struct KeyRange {
    pub StartKey: Key,
    pub EndKey: Key,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 下推扫描 DAG 中的执行器节点。
pub enum ScanExecutor {
    Index(IndexScanPB),
    Table(TableScanPB),
    Limit(u64),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引扫描 protobuf 参数。
pub struct IndexScanPB {
    pub TableId: i64,
    pub IndexId: i64,
    pub Columns: Vec<ColumnInfo>,
    pub PrimaryColumnIds: Vec<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表扫描 protobuf 参数。
pub struct TableScanPB {
    pub TableId: i64,
    pub Columns: Vec<ColumnInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 下推给 TiKV 的 DAG 请求。
pub struct DAGRequest {
    pub TimeZoneName: String,
    pub TimeZoneOffset: i64,
    pub Flags: u64,
    pub OutputOffsets: Vec<u32>,
    pub Executors: Vec<ScanExecutor>,
    pub EncodeType: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// ADMIN 扫描用途分类。
pub enum ScanKind {
    CheckIndex,
    RecoverTable,
    CleanupIndex,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 构造 Select/扫描请求的参数包。
pub struct SelectRequest {
    pub kind: ScanKind,
    pub table_id: i64,
    pub index_id: Option<i64>,
    pub start_ts: u64,
    pub key_ranges: Vec<KeyRange>,
    pub keep_order: bool,
    pub concurrency: usize,
    pub dag: DAGRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 待写入/删除的索引条目。
pub struct IndexEntry {
    pub key: Key,
    pub value: Vec<u8>,
    pub distinct: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 创建索引时的选项（忽略断言、跳过重复检查）。
pub struct IndexCreateOptions {
    pub ignore_assertion: bool,
    pub duplicate_check_skip: bool,
}

/// 下推扫描结果迭代器。
pub trait AdminSelectResult: Send {
    fn Next(&mut self, output: &mut AdminChunk) -> AdapterResult;
    fn Close(&mut self) -> AdapterResult;
}

/// ADMIN 路径使用的事务能力：批量读、加锁、创建/删除索引。
pub trait AdminTransaction {
    fn StartTS(&self) -> u64;
    fn SetTopSQLOption(&mut self) -> AdapterResult;
    fn SetDiskFullAllowedOnAlmostFull(&mut self) -> AdapterResult;
    fn BatchGetValue(&mut self, keys: &[Key]) -> AdapterResult<HashMap<Key, Vec<u8>>>;
    fn LockKey(&mut self, key: &[u8]) -> AdapterResult;
    fn CreateIndex(
        &mut self,
        table: &TableInfo,
        index: &IndexInfo,
        physical_id: i64,
        values: &[Datum],
        handle: &Handle,
        restored_data: &[Datum],
        options: IndexCreateOptions,
    ) -> AdapterResult;
    fn DeleteIndex(
        &mut self,
        table: &TableInfo,
        index: &IndexInfo,
        physical_id: i64,
        values: &[Datum],
        handle: &Handle,
    ) -> AdapterResult;
}

/// ADMIN 运行时：构造扫描、键编码、分区解析与进度日志。
pub trait AdminRuntime: Send + Sync {
    fn Location(&self) -> (String, i64);
    fn PushDownFlags(&self) -> u64;
    fn EncodeType(&self) -> String;
    fn ActivateTransaction(&self) -> AdapterResult<u64>;
    fn FullIndexRange(&self, table: &TableInfo, index: &IndexInfo) -> AdapterResult<KeyRange>;
    fn IndexRangeAfter(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        physical_id: i64,
        last_index_key: &[u8],
    ) -> AdapterResult<KeyRange>;
    fn ValidateColumnDefaults(&self, columns: &[ColumnInfo]) -> AdapterResult;
    fn Select(&self, request: SelectRequest) -> AdapterResult<Box<dyn AdminSelectResult>>;
    fn RunInNewTxn(
        &self,
        operation: &mut dyn FnMut(&mut dyn AdminTransaction) -> AdapterResult,
    ) -> AdapterResult;
    fn OpenBaseExecutor(&self) -> AdapterResult;
    fn CloseBaseExecutor(&self) -> AdapterResult;
    fn BuildHandle(&self, row: &Row, handle_columns: &[usize]) -> AdapterResult<Handle>;
    fn MeetPartialCondition(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        row: &Row,
    ) -> AdapterResult<bool>;
    fn EvalGeneratedColumn(
        &self,
        table: &TableInfo,
        column: &ColumnInfo,
        row: &Row,
    ) -> AdapterResult<Datum>;
    fn RestoredData(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        row: &Row,
        handle: &Handle,
    ) -> AdapterResult<Vec<Datum>>;
    fn GenIndexEntries(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        values: &[Datum],
        handle: &Handle,
    ) -> AdapterResult<Vec<IndexEntry>>;
    fn DecodeHandleInIndexValue(&self, value: &[u8]) -> AdapterResult<Handle>;
    fn EncodeRecordKey(&self, table: &TableInfo, handle: &Handle) -> Key;
    fn DecodeRecordKey(&self, key: &[u8]) -> AdapterResult<(i64, Handle)>;
    fn RecordPrefix(&self, table_id: i64) -> Key;
    fn ResolvePartition(&self, table: &TableInfo, partition_id: i64) -> AdapterResult<TableInfo>;
    fn ResolveWritableIndex(&self, table: &TableInfo, index_name: &str)
    -> AdapterResult<IndexInfo>;
    fn MinimumIndexKey(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        physical_id: i64,
    ) -> AdapterResult<Key>;
    fn LogRecoverProgress(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        added: i64,
        scanned: i64,
        handle: &Handle,
    );
    fn LogUniqueIndexMismatch(
        &self,
        index: &IndexInfo,
        key: &[u8],
        table_handle: &Handle,
        index_handle: &Handle,
    );
    fn LogCleanupProgress(&self, table: &TableInfo, index: &IndexInfo, removed: u64);
}

/// 关闭 select 结果并优先返回业务错误。
fn finish_select_result<T>(
    result: AdapterResult<T>,
    select: &mut dyn AdminSelectResult,
) -> AdapterResult<T> {
    let close = select.Close();
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
    }
}

/// 计算字典序上“严格大于 key 前缀”的下一个键（范围上界）。
fn prefix_next(mut key: Key) -> Key {
    for index in (0..key.len()).rev() {
        if key[index] != u8::MAX {
            key[index] += 1;
            key.truncate(index + 1);
            return key;
        }
    }
    key.push(0);
    key
}

/// `ADMIN CHECK INDEX`：扫描索引并过滤落在 handleRanges 内的行。
pub struct CheckIndexRangeExec {
    pub runtime: Arc<dyn AdminRuntime>,
    pub table: TableInfo,
    pub index: IndexInfo,
    pub startKey: Vec<Datum>,
    pub handleRanges: Vec<HandleRange>,
    pub result: Option<Box<dyn AdminSelectResult>>,
    pub cols: Vec<ColumnInfo>,
    pub output_columns: usize,
}

/// CheckIndexRangeExec：打开索引扫描、按 handle 范围过滤输出。
impl CheckIndexRangeExec {
    pub fn Next(&mut self, req: &mut AdminChunk) -> AdapterResult {
        req.Reset();
        let handle_index = self
            .output_columns
            .checked_sub(1)
            .ok_or_else(|| errors::New("check-index schema has no handle column"))?;
        loop {
            let mut source = AdminChunk::default();
            self.result
                .as_mut()
                .ok_or_else(|| errors::New("check-index executor is not open"))?
                .Next(&mut source)?;
            if source.NumRows() == 0 {
                return Ok(());
            }
            let mut rows = Vec::with_capacity(source.NumRows());
            for row in source.rows {
                let handle = row.datum(handle_index)?.as_i64()?;
                if self
                    .handleRanges
                    .iter()
                    .any(|range| handle >= range.Begin && handle < range.End)
                {
                    rows.push(row);
                }
            }
            req.AppendRows(rows);
            if req.NumRows() > 0 {
                return Ok(());
            }
        }
    }

    pub fn Open(&mut self) -> AdapterResult {
        self.cols.clear();
        for index_column in &self.index.Columns {
            let column = self.table.Columns.get(index_column.Offset).ok_or_else(|| {
                errors::New(format!(
                    "index column offset {} is outside table schema",
                    index_column.Offset
                ))
            })?;
            self.cols.push(column.clone());
        }
        self.cols.push(ColumnInfo {
            ID: -1,
            Name: "_tidb_rowid".to_owned(),
            FieldType: FieldType {
                name: "BIGINT".to_owned(),
                array_element: None,
            },
            ..ColumnInfo::default()
        });
        let dag = self.buildDAGPB()?;
        let start_ts = self.runtime.ActivateTransaction()?;
        let request = SelectRequest {
            kind: ScanKind::CheckIndex,
            table_id: self.table.ID,
            index_id: Some(self.index.ID),
            start_ts,
            key_ranges: vec![self.runtime.FullIndexRange(&self.table, &self.index)?],
            keep_order: true,
            concurrency: 0,
            dag,
        };
        self.result = Some(self.runtime.Select(request)?);
        Ok(())
    }

    pub fn buildDAGPB(&self) -> AdapterResult<DAGRequest> {
        self.runtime.ValidateColumnDefaults(&self.cols)?;
        let (timezone_name, timezone_offset) = self.runtime.Location();
        Ok(DAGRequest {
            TimeZoneName: timezone_name,
            TimeZoneOffset: timezone_offset,
            Flags: self.runtime.PushDownFlags(),
            OutputOffsets: (0..self.output_columns).map(|index| index as u32).collect(),
            Executors: vec![self.constructIndexScanPB()],
            EncodeType: self.runtime.EncodeType(),
        })
    }

    pub fn constructIndexScanPB(&self) -> ScanExecutor {
        ScanExecutor::Index(IndexScanPB {
            TableId: self.table.ID,
            IndexId: self.index.ID,
            Columns: self.cols.clone(),
            PrimaryColumnIds: Vec::new(),
        })
    }

    pub fn Close(&mut self) -> AdapterResult {
        let result = self.result.as_mut().map_or(Ok(()), |result| result.Close());
        self.result = None;
        result
    }
}

/// `ADMIN RECOVER INDEX`：扫描表回填缺失的索引项。
pub struct RecoverIndexExec {
    pub runtime: Arc<dyn AdminRuntime>,
    pub done: bool,
    pub index: IndexInfo,
    pub table: TableInfo,
    pub physicalID: i64,
    pub batchSize: usize,
    pub columns: Vec<ColumnInfo>,
    pub colFieldTypes: Vec<FieldType>,
    pub handleCols: Vec<usize>,
    pub containsGenedColOrPartialIndex: bool,
    pub recoverRows: Vec<recoverRows>,
    pub idxValsBufs: Vec<Vec<Datum>>,
    pub idxKeyBufs: Vec<Vec<u8>>,
    pub batchKeys: Vec<Key>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单次回填批次的进度：当前 handle、新增数、扫描行数。
pub struct backfillResult {
    pub currentHandle: Option<Handle>,
    pub addedCount: i64,
    pub scanRowCount: i64,
}

impl Default for backfillResult {
    fn default() -> Self {
        Self {
            currentHandle: None,
            addedCount: 0,
            scanRowCount: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 待恢复的一行：handle、索引列值与还原数据。
pub struct recoverRows {
    pub handle: Handle,
    pub idxVals: Vec<Datum>,
    pub rsData: Vec<Datum>,
    pub skip: bool,
}

/// RecoverIndexExec：批量表扫描、去重标记、事务内写回索引。
impl RecoverIndexExec {
    /// 懒构建并返回回填所需列类型列表。
    pub fn columnsTypes(&mut self) -> &[FieldType] {
        if self.colFieldTypes.is_empty() {
            self.colFieldTypes = self
                .columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect();
        }
        &self.colFieldTypes
    }

    pub fn Open(&mut self) -> AdapterResult {
        self.runtime.OpenBaseExecutor()?;
        self.columnsTypes();
        self.batchSize = 2048;
        self.recoverRows = Vec::with_capacity(self.batchSize);
        self.idxValsBufs = vec![Vec::new(); self.batchSize];
        self.idxKeyBufs = vec![Vec::new(); self.batchSize];
        Ok(())
    }

    /// 构造表扫描 PB 节点。
    pub fn constructTableScanPB(
        &self,
        _table_info: &TableInfo,
        column_infos: &[ColumnInfo],
    ) -> AdapterResult<ScanExecutor> {
        Ok(ScanExecutor::Table(TableScanPB {
            TableId: self.physicalID,
            Columns: column_infos.to_vec(),
        }))
    }

    /// 构造 Limit 下推节点。
    pub fn constructLimitPB(&self, count: u64) -> ScanExecutor {
        ScanExecutor::Limit(count)
    }

    pub fn buildDAGPB(
        &self,
        _transaction: &mut dyn AdminTransaction,
        limit_count: u64,
    ) -> AdapterResult<DAGRequest> {
        self.runtime.ValidateColumnDefaults(&self.columns)?;
        let (timezone_name, timezone_offset) = self.runtime.Location();
        Ok(DAGRequest {
            TimeZoneName: timezone_name,
            TimeZoneOffset: timezone_offset,
            Flags: self.runtime.PushDownFlags(),
            OutputOffsets: (0..self.columns.len()).map(|index| index as u32).collect(),
            Executors: vec![
                self.constructTableScanPB(&self.table, &self.columns)?,
                self.constructLimitPB(limit_count),
            ],
            EncodeType: self.runtime.EncodeType(),
        })
    }

    /// 打开从表扫描起始的 Select 结果。
    pub fn buildTableScan(
        &self,
        transaction: &mut dyn AdminTransaction,
        start_handle: Option<&Handle>,
        limit_count: u64,
    ) -> AdapterResult<Box<dyn AdminSelectResult>> {
        let request = SelectRequest {
            kind: ScanKind::RecoverTable,
            table_id: self.physicalID,
            index_id: None,
            start_ts: transaction.StartTS(),
            key_ranges: buildRecoverIndexKeyRanges(
                self.runtime.as_ref(),
                self.physicalID,
                start_handle,
            )?,
            keep_order: true,
            concurrency: 1,
            dag: self.buildDAGPB(transaction, limit_count)?,
        };
        self.runtime.Select(request)
    }

    /// 循环回填直至完成，返回 (added, scanned)。
    pub fn backfillIndex(&mut self) -> AdapterResult<(i64, i64)> {
        let mut current_handle: Option<Handle> = None;
        let mut total_added = 0_i64;
        let mut total_scanned = 0_i64;
        let mut last_logged = 0_i64;
        loop {
            let mut transaction_result: Option<backfillResult> = None;
            let runtime = Arc::clone(&self.runtime);
            let mut operation = |transaction: &mut dyn AdminTransaction| {
                transaction.SetTopSQLOption()?;
                let result = self.backfillIndexInTxn(transaction, current_handle.as_ref())?;
                transaction_result = Some(result);
                Ok(())
            };
            runtime.RunInNewTxn(&mut operation)?;
            let result = transaction_result.ok_or_else(|| {
                errors::New("recover-index transaction completed without a backfill result")
            })?;
            total_added += result.addedCount;
            total_scanned += result.scanRowCount;
            if total_scanned - last_logged >= 50_000 {
                last_logged = total_scanned;
                if let Some(handle) = result.currentHandle.as_ref() {
                    self.runtime.LogRecoverProgress(
                        &self.table,
                        &self.index,
                        total_added,
                        total_scanned,
                        handle,
                    );
                }
            }
            if result.scanRowCount == 0 {
                break;
            }
            let handle = result.currentHandle.ok_or_else(|| {
                errors::New("recover-index scan returned rows without a current handle")
            })?;
            if handle.successor().is_none() {
                break;
            }
            current_handle = Some(handle);
        }
        Ok((total_added, total_scanned))
    }

    /// 从扫描结果取出一批待恢复行。
    pub fn fetchRecoverRows(
        &mut self,
        source: &mut dyn AdminSelectResult,
        result: &mut backfillResult,
    ) -> AdapterResult<Vec<recoverRows>> {
        self.recoverRows.clear();
        let index_value_len = self.index.Columns.len();
        result.scanRowCount = 0;
        loop {
            let mut chunk = AdminChunk::default();
            source.Next(&mut chunk)?;
            if chunk.NumRows() == 0 {
                break;
            }
            for row in chunk.rows {
                if result.scanRowCount >= self.batchSize as i64 {
                    return Ok(self.recoverRows.clone());
                }
                let base_handle = self.runtime.BuildHandle(&row, &self.handleCols)?;
                let handle = if self.index.Global {
                    Handle::partitioned(self.physicalID, &base_handle)
                } else {
                    base_handle
                };
                if self.index.HasCondition
                    && !self
                        .runtime
                        .MeetPartialCondition(&self.table, &self.index, &row)?
                {
                    result.scanRowCount += 1;
                    result.currentHandle = Some(handle);
                    continue;
                }
                let buffer_index = result.scanRowCount as usize;
                let buffer = self
                    .idxValsBufs
                    .get(buffer_index)
                    .cloned()
                    .unwrap_or_default();
                let values = self.buildIndexedValues(&row, buffer, index_value_len)?;
                if buffer_index < self.idxValsBufs.len() {
                    self.idxValsBufs[buffer_index] = values.clone();
                }
                let restored_data =
                    self.runtime
                        .RestoredData(&self.table, &self.index, &row, &handle)?;
                self.recoverRows.push(recoverRows {
                    handle: handle.clone(),
                    idxVals: values,
                    rsData: restored_data,
                    skip: true,
                });
                result.scanRowCount += 1;
                result.currentHandle = Some(handle);
            }
        }
        Ok(self.recoverRows.clone())
    }

    /// 计算生成列/部分索引条件并填充索引列值缓冲。
    pub fn buildIndexedValues(
        &self,
        row: &Row,
        mut index_values: Vec<Datum>,
        index_value_len: usize,
    ) -> AdapterResult<Vec<Datum>> {
        if !self.containsGenedColOrPartialIndex {
            return extractIdxVals(row, index_values, &self.colFieldTypes, index_value_len);
        }
        index_values.clear();
        index_values.reserve(index_value_len);
        for index_column in &self.index.Columns {
            let column = self.table.Columns.get(index_column.Offset).ok_or_else(|| {
                errors::New(format!(
                    "index column offset {} is outside table schema",
                    index_column.Offset
                ))
            })?;
            let value = if self.containsGenedColOrPartialIndex && column.Generated {
                self.runtime.EvalGeneratedColumn(&self.table, column, row)?
            } else {
                row.datum(index_column.Offset)?.clone()
            };
            index_values.push(value);
        }
        Ok(index_values)
    }

    /// 批量探测已存在索引键，标记重复以免重复写入。
    pub fn batchMarkDup(
        &mut self,
        transaction: &mut dyn AdminTransaction,
        rows: &mut [recoverRows],
    ) -> AdapterResult {
        if rows.is_empty() {
            return Ok(());
        }
        self.batchKeys.clear();
        let mut distinct_flags = Vec::new();
        let mut row_indexes = Vec::new();
        for (row_index, row) in rows.iter().enumerate() {
            for entry in
                self.runtime
                    .GenIndexEntries(&self.table, &self.index, &row.idxVals, &row.handle)?
            {
                self.batchKeys.push(entry.key);
                distinct_flags.push(entry.distinct);
                row_indexes.push(row_index);
            }
        }
        let values = transaction.BatchGetValue(&self.batchKeys)?;
        for (entry_index, key) in self.batchKeys.iter().enumerate() {
            let row_index = row_indexes[entry_index];
            let found = values.get(key);
            if let Some(value) = found
                && distinct_flags[entry_index]
            {
                let index_handle = self.runtime.DecodeHandleInIndexValue(value)?;
                if index_handle != rows[row_index].handle {
                    self.runtime.LogUniqueIndexMismatch(
                        &self.index,
                        key,
                        &rows[row_index].handle,
                        &index_handle,
                    );
                }
            }
            rows[row_index].skip = found.is_some() && rows[row_index].skip;
        }
        Ok(())
    }

    /// 在新事务中批量 CreateIndex。
    pub fn backfillIndexInTxn(
        &mut self,
        transaction: &mut dyn AdminTransaction,
        current_handle: Option<&Handle>,
    ) -> AdapterResult<backfillResult> {
        let mut source = self.buildTableScan(transaction, current_handle, self.batchSize as u64)?;
        let mut result = backfillResult::default();
        let fetch = self.fetchRecoverRows(source.as_mut(), &mut result);
        let mut rows = finish_select_result(fetch, source.as_mut())?;
        self.batchMarkDup(transaction, &mut rows)?;
        for row in rows {
            if row.skip {
                continue;
            }
            let record_key = self.runtime.EncodeRecordKey(&self.table, &row.handle);
            transaction.LockKey(&record_key)?;
            transaction.CreateIndex(
                &self.table,
                &self.index,
                self.physicalID,
                &row.idxVals,
                &row.handle,
                &row.rsData,
                IndexCreateOptions {
                    ignore_assertion: true,
                    duplicate_check_skip: true,
                },
            )?;
            result.addedCount += 1;
        }
        Ok(result)
    }

    pub fn Next(&mut self, req: &mut AdminChunk) -> AdapterResult {
        req.Reset();
        if self.done {
            return Ok(());
        }
        if self.index.Primary && self.table.IsCommonHandle {
            req.AppendInt64(0, 0);
            req.AppendInt64(1, 0);
            self.done = true;
            return Ok(());
        }
        let mut total_added = 0_i64;
        let mut total_scanned = 0_i64;
        if self.table.Partitions.is_empty() {
            (total_added, total_scanned) = self.backfillIndex()?;
        } else {
            let base_table = self.table.clone();
            let index_name = self.index.Name.clone();
            for partition in &base_table.Partitions {
                self.table = self.runtime.ResolvePartition(&base_table, partition.ID)?;
                self.index = self
                    .runtime
                    .ResolveWritableIndex(&self.table, &index_name)?;
                self.physicalID = partition.ID;
                let (added, scanned) = self.backfillIndex()?;
                total_added += added;
                total_scanned += scanned;
            }
        }
        req.AppendInt64(0, total_added);
        req.AppendInt64(1, total_scanned);
        self.done = true;
        Ok(())
    }

    pub fn Close(&mut self) -> AdapterResult {
        self.runtime.CloseBaseExecutor()
    }
}

/// 构造恢复索引用的表键范围（整表或从某 handle 之后）。
pub fn buildRecoverIndexKeyRanges(
    runtime: &dyn AdminRuntime,
    table_id: i64,
    start_handle: Option<&Handle>,
) -> AdapterResult<Vec<KeyRange>> {
    let prefix = runtime.RecordPrefix(table_id);
    let start = match start_handle {
        Some(handle) => prefix_next(runtime.EncodeRecordKey(
            &TableInfo {
                ID: table_id,
                ..TableInfo::default()
            },
            handle,
        )),
        None => prefix_next(prefix.clone()),
    };
    Ok(vec![KeyRange {
        StartKey: start,
        EndKey: prefix_next(prefix),
    }])
}

/// `ADMIN CLEANUP INDEX`：扫描索引并删除表中已不存在的悬空项。
pub struct CleanupIndexExec {
    pub runtime: Arc<dyn AdminRuntime>,
    pub done: bool,
    pub removeCnt: u64,
    pub index: IndexInfo,
    pub table: TableInfo,
    pub physicalID: i64,
    pub columns: Vec<ColumnInfo>,
    pub idxColFieldTypes: Vec<FieldType>,
    pub handleCols: Vec<usize>,
    pub idxValues: HashMap<Handle, Vec<Vec<Datum>>>,
    pub batchSize: u64,
    pub batchKeys: Vec<Key>,
    pub idxValsBufs: Vec<Vec<Datum>>,
    pub lastIdxKey: Key,
    pub scanRowCnt: u64,
}

/// CleanupIndexExec：索引扫描、批量查表、删除悬空索引。
impl CleanupIndexExec {
    /// 懒构建索引列类型。
    pub fn getIdxColTypes(&mut self) -> &[FieldType] {
        if self.idxColFieldTypes.is_empty() {
            self.idxColFieldTypes = self
                .columns
                .iter()
                .map(|column| column.FieldType.ArrayType())
                .collect();
        }
        &self.idxColFieldTypes
    }

    /// 批量读取记录键以判断索引是否悬空。
    pub fn batchGetRecord(
        &mut self,
        transaction: &mut dyn AdminTransaction,
    ) -> AdapterResult<HashMap<Key, Vec<u8>>> {
        self.batchKeys.clear();
        self.batchKeys.extend(
            self.idxValues
                .keys()
                .map(|handle| self.runtime.EncodeRecordKey(&self.table, handle)),
        );
        transaction.BatchGetValue(&self.batchKeys)
    }

    /// 删除确认悬空的索引项。
    pub fn deleteDanglingIdx(
        &mut self,
        transaction: &mut dyn AdminTransaction,
        values: &HashMap<Key, Vec<u8>>,
    ) -> AdapterResult {
        for key in &self.batchKeys {
            if values.contains_key(key) {
                continue;
            }
            let (partition_id, decoded_handle) = self.runtime.DecodeRecordKey(key)?;
            let handle = if self.index.Global {
                Handle::partitioned(partition_id, &decoded_handle)
            } else {
                decoded_handle
            };
            let value_groups = self.idxValues.get(&handle).ok_or_else(|| {
                errors::New("batch record keys are inconsistent with scanned index handles")
            })?;
            for index_values in value_groups {
                transaction.DeleteIndex(
                    &self.table,
                    &self.index,
                    self.physicalID,
                    index_values,
                    &handle,
                )?;
                self.removeCnt += 1;
                if self.batchSize != 0 && self.removeCnt % self.batchSize == 0 {
                    self.runtime
                        .LogCleanupProgress(&self.table, &self.index, self.removeCnt);
                }
            }
        }
        Ok(())
    }

    /// 拉取下一批索引行到内部缓冲。
    pub fn fetchIndex(&mut self, transaction: &mut dyn AdminTransaction) -> AdapterResult {
        let mut result = self.buildIndexScan(transaction)?;
        let fetch = (|| -> AdapterResult {
            let index_column_count = self.index.Columns.len();
            loop {
                let mut chunk = AdminChunk::default();
                result.Next(&mut chunk)?;
                if chunk.NumRows() == 0 {
                    return Ok(());
                }
                for row in chunk.rows {
                    let base_handle = self.runtime.BuildHandle(&row, &self.handleCols)?;
                    let handle = if self.index.Global {
                        let partition_id = row
                            .values
                            .last()
                            .ok_or_else(|| errors::New("global index row has no partition id"))?
                            .as_i64()?;
                        Handle::partitioned(partition_id, &base_handle)
                    } else {
                        base_handle
                    };
                    let buffer = self
                        .idxValsBufs
                        .get(self.scanRowCnt as usize)
                        .cloned()
                        .unwrap_or_default();
                    let index_values =
                        extractIdxVals(&row, buffer, &self.idxColFieldTypes, index_column_count)?;
                    if (self.scanRowCnt as usize) < self.idxValsBufs.len() {
                        self.idxValsBufs[self.scanRowCnt as usize] = index_values.clone();
                    }
                    self.idxValues
                        .entry(handle.clone())
                        .or_default()
                        .push(index_values.clone());
                    let entry = self
                        .runtime
                        .GenIndexEntries(&self.table, &self.index, &index_values, &handle)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| errors::New("index generated no key for scanned row"))?;
                    self.scanRowCnt += 1;
                    self.lastIdxKey = entry.key;
                    if self.scanRowCnt >= self.batchSize {
                        return Ok(());
                    }
                }
            }
        })();
        finish_select_result(fetch, result.as_mut())
    }

    pub fn Next(&mut self, req: &mut AdminChunk) -> AdapterResult {
        req.Reset();
        if self.done {
            return Ok(());
        }
        if self.table.IsCommonHandle && self.index.Primary {
            self.done = true;
            req.AppendUint64(0, 0);
            return Ok(());
        }
        if !self.table.Partitions.is_empty() && !self.index.Global {
            let base_table = self.table.clone();
            let index_name = self.index.Name.clone();
            for partition in &base_table.Partitions {
                self.table = self.runtime.ResolvePartition(&base_table, partition.ID)?;
                self.index = self
                    .runtime
                    .ResolveWritableIndex(&self.table, &index_name)?;
                self.physicalID = partition.ID;
                self.init()?;
                self.cleanTableIndex()?;
            }
        } else {
            self.cleanTableIndex()?;
        }
        self.done = true;
        req.AppendUint64(0, self.removeCnt);
        Ok(())
    }

    /// 清理整表（或分区）悬空索引的主循环。
    pub fn cleanTableIndex(&mut self) -> AdapterResult {
        loop {
            let runtime = Arc::clone(&self.runtime);
            let mut operation = |transaction: &mut dyn AdminTransaction| {
                transaction.SetDiskFullAllowedOnAlmostFull()?;
                transaction.SetTopSQLOption()?;
                self.fetchIndex(transaction)?;
                let values = self.batchGetRecord(transaction)?;
                self.deleteDanglingIdx(transaction, &values)
            };
            runtime.RunInNewTxn(&mut operation)?;
            if self.scanRowCnt == 0 {
                break;
            }
            self.scanRowCnt = 0;
            self.batchKeys.clear();
            self.idxValues.clear();
        }
        Ok(())
    }

    /// 构造索引扫描 Select 请求。
    pub fn buildIndexScan(
        &self,
        transaction: &mut dyn AdminTransaction,
    ) -> AdapterResult<Box<dyn AdminSelectResult>> {
        let index_range = self.runtime.IndexRangeAfter(
            &self.table,
            &self.index,
            self.physicalID,
            &self.lastIdxKey,
        )?;
        let request = SelectRequest {
            kind: ScanKind::CleanupIndex,
            table_id: self.physicalID,
            index_id: Some(self.index.ID),
            start_ts: transaction.StartTS(),
            key_ranges: vec![index_range],
            keep_order: true,
            concurrency: 1,
            dag: self.buildIdxDAGPB()?,
        };
        self.runtime.Select(request)
    }

    pub fn Open(&mut self) -> AdapterResult {
        self.runtime.OpenBaseExecutor()?;
        let result = self.init();
        if result.is_err() {
            let _ = self.runtime.CloseBaseExecutor();
        }
        result
    }

    /// 初始化 CleanupIndex：解析可写索引与列类型。
    pub fn init(&mut self) -> AdapterResult {
        if self.batchSize == 0 {
            return Err(errors::New("cleanup-index batch size must be positive"));
        }
        self.getIdxColTypes();
        self.idxValues.clear();
        self.batchKeys = Vec::with_capacity(self.batchSize as usize);
        self.idxValsBufs = vec![Vec::new(); self.batchSize as usize];
        self.lastIdxKey =
            self.runtime
                .MinimumIndexKey(&self.table, &self.index, self.physicalID)?;
        self.scanRowCnt = 0;
        Ok(())
    }

    /// 构造索引扫描 DAG。
    pub fn buildIdxDAGPB(&self) -> AdapterResult<DAGRequest> {
        self.runtime.ValidateColumnDefaults(&self.columns)?;
        let (timezone_name, timezone_offset) = self.runtime.Location();
        Ok(DAGRequest {
            TimeZoneName: timezone_name,
            TimeZoneOffset: timezone_offset,
            Flags: self.runtime.PushDownFlags(),
            OutputOffsets: (0..self.columns.len()).map(|index| index as u32).collect(),
            Executors: vec![self.constructIndexScanPB(), self.constructLimitPB()],
            EncodeType: self.runtime.EncodeType(),
        })
    }

    pub fn constructIndexScanPB(&self) -> ScanExecutor {
        let primary_columns = if self.table.IsCommonHandle {
            self.table.PrimaryColumnIds.clone()
        } else {
            Vec::new()
        };
        ScanExecutor::Index(IndexScanPB {
            TableId: self.physicalID,
            IndexId: self.index.ID,
            Columns: self.columns.clone(),
            PrimaryColumnIds: primary_columns,
        })
    }

    pub fn constructLimitPB(&self) -> ScanExecutor {
        ScanExecutor::Limit(self.batchSize)
    }

    pub fn Close(&mut self) -> AdapterResult {
        self.runtime.CloseBaseExecutor()
    }
}

/// 从行中按索引列 Offset 提取索引列 Datum 列表。
pub fn extractIdxVals(
    row: &Row,
    mut index_values: Vec<Datum>,
    field_types: &[FieldType],
    index_value_len: usize,
) -> AdapterResult<Vec<Datum>> {
    if field_types.len() < index_value_len {
        return Err(errors::New(
            "index field types do not cover all index values",
        ));
    }
    if row.values.len() < index_value_len {
        return Err(errors::New("index row does not cover all index values"));
    }
    index_values.clear();
    index_values.extend(row.values.iter().take(index_value_len).cloned());
    Ok(index_values)
}
