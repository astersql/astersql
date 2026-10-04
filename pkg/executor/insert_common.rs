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

// INSERT 公共逻辑：值求值、默认值、自增/自随机、重复键批检与错误补全。
//
// 通过 `InsertBackend` 抽象会话/表/表达式能力，`InsertValues` 持有列与缓冲状态，
// `insertCommon` 实现 fillRow、auto increment（自增）、ON DUPLICATE 冲突处理等；
// `insertRows` / `insertRowsFromSelect` 为 VALUES 与 SELECT 两条插入主路径。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::fmt::Write;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// INSERT/LOAD 值转换与约束相关错误种类。
pub enum InsertErrorKind {
    DataTooLong,
    Overflow,
    Truncated,
    TruncatedWrongValue,
    WrongValue,
    WarnDataOutOfRange,
    TimestampInDstTransition,
    NoDefaultValue,
    CheckConstraintViolated,
    NotFound,
    Other,
}

/// Stable error cause accepted by the narrow DML formatting boundary.
/// DML 格式化边界接受的稳定错误原因。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DmlErrorCause {
    kind: InsertErrorKind,
    message: String,
}

impl DmlErrorCause {
    /// 构造带种类与消息的 DML 错误原因。
    pub fn new(kind: InsertErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 返回错误种类。
    pub fn kind(&self) -> InsertErrorKind {
        self.kind
    }
}

impl std::fmt::Display for DmlErrorCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DmlErrorCause {}

/// Completed user-facing DML error with its original conversion cause.
/// 面向用户的完整 DML 错误，保留原始转换 cause。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedDmlError {
    kind: InsertErrorKind,
    message: String,
    cause: DmlErrorCause,
}

impl CompletedDmlError {
    /// 返回完整错误的种类。
    pub fn kind(&self) -> InsertErrorKind {
        self.kind
    }
}

impl std::fmt::Display for CompletedDmlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CompletedDmlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// Formats INSERT conversion errors with Go's one-based row number while
/// retaining the low-level conversion error as the source.
/// 按 Go 的一行号格式化 INSERT 转换错误，并保留底层 cause。
pub fn CompleteInsertErrorForColumn(
    column: &str,
    row_index: usize,
    cause: DmlErrorCause,
) -> CompletedDmlError {
    let kind = cause.kind();
    let row = row_index.saturating_add(1);
    // 按错误种类生成带列名与一行号的消息
    let message = match kind {
        InsertErrorKind::DataTooLong => {
            format!("Data too long for column '{column}' at row {row}")
        }
        InsertErrorKind::Overflow | InsertErrorKind::WarnDataOutOfRange => {
            format!("Out of range value for column '{column}' at row {row}")
        }
        InsertErrorKind::Truncated => {
            format!("Data truncated for column '{column}' at row {row}")
        }
        _ => cause.to_string(),
    };
    CompletedDmlError {
        kind,
        message,
        cause,
    }
}

/// Formats LOAD DATA conversion errors. Go keeps LOAD's caller-provided row
/// index and rewrites DataTooLong to DataTruncated; other classifications pass
/// through unchanged. The conversion cause remains available through source.
/// 格式化 LOAD DATA 转换错误；DataTooLong 改写为 Truncated。
pub fn CompleteLoadErrorForColumn(
    column: &str,
    row_index: usize,
    cause: DmlErrorCause,
) -> CompletedDmlError {
    let original_kind = cause.kind();
    // LOAD：DataTooLong 统一改写为 Truncated
    let (kind, message) = if original_kind == InsertErrorKind::DataTooLong {
        (
            InsertErrorKind::Truncated,
            format!("Data truncated for column '{column}' at row {row_index}"),
        )
    } else {
        (original_kind, cause.to_string())
    };
    CompletedDmlError {
        kind,
        message,
        cause,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 列字段类型粗分类（时长/日期/浮点/整数等）。
pub enum FieldKind {
    Duration,
    DateTime,
    Date,
    Timestamp,
    Float,
    Double,
    Integer,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 批检路径上的重复键检查模式：跳过或正常检查。
pub enum DupKeyCheckMode {
    Skip,
    Normal,
}

/// Match Go's `getRow` error selection after the statement error context has
/// either accepted a cast error as a warning or rejected it.
pub(crate) fn resolve_get_row_cast_error<E>(
    load: bool,
    original: E,
    handled: Result<(), E>,
) -> Result<(), E> {
    match handled {
        Ok(()) => Ok(()),
        Err(completed) if load => Err(completed),
        Err(_) => Err(original),
    }
}

#[derive(Clone)]
/// 待检唯一键及其冲突时预构造的错误。
pub struct DuplicateKey<E> {
    pub new_key: Vec<u8>,
    pub duplicate_error: E,
}

#[derive(Clone)]
/// 待做重复键检查的行：数据、表、handle/唯一键与忽略标记。
pub struct ToBeCheckedRow<D, T, E> {
    pub row: Vec<D>,
    pub table: T,
    pub handle_key: Option<DuplicateKey<E>>,
    pub unique_keys: Vec<DuplicateKey<E>>,
    pub ignored: bool,
}

/// A terminal allocator error must survive conversion and INSERT IGNORE handling.
pub(crate) fn is_terminal_auto_id_error(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if let Some(error) = error.downcast_ref::<astersql_meta_autoid::AutoIdError>() {
            if astersql_meta_autoid::is_rpc_retry_limit_error(error) {
                return true;
            }
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

/// INSERT 后端能力边界：错误、列、表达式求值、自增分配、事务写行等。
pub trait InsertBackend: Send + Sync + 'static {
    type Context: Clone;
    type Error: Clone + std::error::Error + 'static;
    type Datum: Clone;
    type Table: Clone;
    type Column: Clone;
    type ColumnName: Clone;
    type Expression: Clone;
    type FieldType: Clone;
    type MutRow;
    type Chunk;
    type Executor: Clone;
    type MemoryTracker: Clone;
    type Transaction: Clone;
    type Handle: Clone;
    type ForeignKeyCheck: Clone;
    type ForeignKeyCascade: Clone;
    type BasicRuntimeStats: Clone;
    type SnapshotRuntimeStats: Clone;
    type AllocatorRuntimeStats: Clone;
    type TraceGuard;

    fn error(&self, message: String) -> Self::Error;
    fn trace_error(&self, error: Self::Error) -> Self::Error;
    fn error_kind(&self, error: &Self::Error) -> InsertErrorKind;
    fn log_value_conversion_error(&self, error: &Self::Error, message: &str);
    fn reset_data_too_long_error(
        &self,
        column: &str,
        row: usize,
        error: Self::Error,
    ) -> Self::Error;
    fn data_out_of_range_error(&self, column: &str, row: usize) -> Self::Error;
    fn truncated_error(&self, column: &str, row: usize) -> Self::Error;
    fn wrong_insert_value_error(
        &self,
        field_kind: FieldKind,
        value: &str,
        column: &str,
        row: usize,
    ) -> Self::Error;
    fn wrong_value_for_field_error(
        &self,
        field_kind: FieldKind,
        value: &str,
        column: &str,
        row: usize,
    ) -> Self::Error;
    fn load_data_truncated_error(&self, column: &str, row: usize) -> Self::Error;
    fn no_default_value_error(&self, column: &Self::Column) -> Self::Error;
    fn invalid_auto_random_error(&self) -> Self::Error;
    fn auto_increment_read_failed(&self) -> Self::Error;
    fn auto_random_read_failed(&self) -> Self::Error;
    fn batch_insert_error(&self, error: Self::Error) -> Self::Error;
    fn old_row_not_found_error(&self, handle: &Self::Handle) -> Self::Error;
    fn functional_index_error(
        &self,
        table: &Self::Table,
        offset: usize,
        row: usize,
        error: Self::Error,
    ) -> Self::Error;

    fn dml_batch_size(&self) -> usize;
    fn batch_insert_enabled(&self) -> bool;
    fn in_transaction(&self) -> bool;
    fn batch_dml_enabled(&self) -> bool;
    fn allow_write_row_id(&self) -> bool;
    fn allow_auto_random_explicit_insert(&self) -> bool;
    fn no_auto_value_on_zero(&self) -> bool;
    fn strict_sql_mode(&self) -> bool;
    fn in_load_data_statement(&self) -> bool;
    fn lock_unchanged_keys(&self) -> bool;
    fn pessimistic_transaction(&self) -> bool;
    fn truncate_as_warning_with_on_duplicate(&self) -> bool;
    fn record_insert_rows_columns_metric(&self, delta: i64);
    fn invalidate_transaction_write_throughput_sli(&self);
    fn set_current_insert_batch_extra_columns(&self, columns: &[Vec<Self::Datum>]);

    fn table_columns(&self, table: &Self::Table) -> Vec<Self::Column>;
    fn table_name(&self, table: &Self::Table) -> String;
    fn table_pk_is_handle(&self, table: &Self::Table) -> bool;
    fn table_is_temporary(&self, table: &Self::Table) -> bool;
    fn table_has_ttl(&self, table: &Self::Table) -> bool;
    fn table_has_exchange_without_partition(&self, table: &Self::Table) -> bool;
    fn requested_column_lower_name(&self, column: &Self::ColumnName) -> String;
    fn requested_column_original_name(&self, column: &Self::ColumnName) -> String;
    fn find_columns(
        &self,
        table_columns: &[Self::Column],
        names: &[String],
        pk_is_handle: bool,
    ) -> (Vec<Self::Column>, Option<usize>);
    fn check_columns_once(&self, columns: &[Self::Column]) -> Result<(), Self::Error>;
    fn column_name(&self, column: &Self::Column) -> String;
    fn column_lower_name(&self, column: &Self::Column) -> String;
    fn column_offset(&self, column: &Self::Column) -> usize;
    fn column_id(&self, column: &Self::Column) -> i64;
    fn column_field_type(&self, column: &Self::Column) -> Self::FieldType;
    fn column_field_kind(&self, column: &Self::Column) -> FieldKind;
    fn field_type_kind(&self, field_type: &Self::FieldType) -> FieldKind;
    fn column_is_generated(&self, column: &Self::Column) -> bool;
    fn column_is_array(&self, column: &Self::Column) -> bool;
    fn column_is_auto_increment(&self, column: &Self::Column) -> bool;
    fn column_has_no_default(&self, column: &Self::Column) -> bool;
    fn column_default_is_expression(&self, column: &Self::Column) -> bool;
    fn column_has_default_expression(&self, column: &Self::Column) -> bool;
    fn table_auto_random_column(&self, table: &Self::Table, column_id: i64) -> bool;
    fn extra_handle_name(&self) -> &str;
    fn extra_handle_id(&self) -> i64;
    fn new_extra_handle_column(&self, offset: usize) -> Self::Column;
    fn longlong_field_type(&self) -> Self::FieldType;

    fn new_mut_row(&self, field_types: &[Self::FieldType]) -> Self::MutRow;
    fn mut_row_set_datums(&self, row: &mut Self::MutRow, datums: &[Self::Datum]);
    fn mut_row_set_datum(&self, row: &mut Self::MutRow, offset: usize, datum: &Self::Datum);
    fn eval_expression(
        &self,
        expression: &Self::Expression,
        row: &Self::MutRow,
    ) -> Result<Self::Datum, Self::Error>;
    fn eval_constant(&self, expression: &Self::Expression) -> Result<Self::Datum, Self::Error>;
    fn cast_value(
        &self,
        datum: &Self::Datum,
        column: &Self::Column,
    ) -> Result<Self::Datum, Self::Error>;
    fn datum_to_string(&self, datum: &Self::Datum) -> Result<String, Self::Error>;
    fn datum_is_null(&self, datum: &Self::Datum) -> bool;
    fn datum_set_null(&self, datum: &mut Self::Datum);
    fn datum_get_int64(&self, datum: &Self::Datum) -> i64;
    fn datum_get_float64(&self, datum: &Self::Datum) -> f64;
    fn datum_set_int64(&self, datum: &mut Self::Datum, value: i64);
    fn datum_set_auto_id(&self, datum: &mut Self::Datum, id: i64, column: &Self::Column);
    fn datum_compare_binary(
        &self,
        left: &Self::Datum,
        right: &Self::Datum,
    ) -> Result<i32, Self::Error>;
    fn null_datum(&self) -> Self::Datum;
    fn zero_value(&self, column: &Self::Column) -> Self::Datum;

    fn warning_count(&self) -> usize;
    fn take_warnings_since(&self, count: usize) -> Vec<Self::Error>;
    fn append_warnings(&self, warnings: Vec<Self::Error>);
    fn append_warning(&self, warning: Self::Error);
    fn handle_statement_error(&self, error: Self::Error) -> Result<(), Self::Error>;
    fn handle_truncate(&self, error: Self::Error) -> Result<(), Self::Error>;
    fn handle_bad_null(
        &self,
        column: &Self::Column,
        datum: &mut Self::Datum,
        load_row_count: u64,
    ) -> Result<(), Self::Error>;
    fn check_exchange_partition_row(
        &self,
        table: &Self::Table,
        row: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    fn eval_default_expression(&self, column: &Self::Column) -> Result<Self::Datum, Self::Error>;
    fn check_no_default_for_insert(&self, column: &Self::Column) -> Result<(), Self::Error>;
    fn column_default_value(&self, column: &Self::Column) -> Result<Self::Datum, Self::Error>;

    fn memory_consume(&self, tracker: &Self::MemoryTracker, bytes: i64);
    fn estimated_rows_memory(&self, first_row: &[Self::Datum], rows: usize) -> i64;
    fn select_executor(&self) -> Self::Executor;
    fn executor_field_types(&self, executor: &Self::Executor) -> Vec<Self::FieldType>;
    fn new_executor_chunk(&self, executor: &Self::Executor) -> Self::Chunk;
    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_memory_usage(&self, chunk: &Self::Chunk) -> i64;
    fn executor_next_rows(
        &self,
        context: &Self::Context,
        executor: &Self::Executor,
        chunk: &mut Self::Chunk,
        field_types: &[Self::FieldType],
    ) -> Result<Vec<Vec<Self::Datum>>, Self::Error>;
    fn statement_commit(&self, context: &Self::Context);
    fn new_transaction_in_statement(&self, context: &Self::Context) -> Result<(), Self::Error>;

    fn retrying(&self) -> bool;
    fn next_retry_auto_increment_id(&self) -> Option<i64>;
    fn next_retry_auto_random_id(&self) -> Option<i64>;
    fn add_retry_auto_increment_id(&self, id: i64);
    fn add_retry_auto_random_id(&self, id: i64);
    fn rebase_auto_increment(
        &self,
        context: &Self::Context,
        table: &Self::Table,
        id: i64,
    ) -> Result<(), Self::Error>;
    fn alloc_batch_auto_increment(
        &self,
        context: &Self::Context,
        table: &Self::Table,
        count: usize,
    ) -> Result<(i64, i64), Self::Error>;
    fn alloc_auto_increment(
        &self,
        context: &Self::Context,
        table: &Self::Table,
    ) -> Result<i64, Self::Error>;
    fn alloc_auto_random_incremental(
        &self,
        context: &Self::Context,
        table: &Self::Table,
    ) -> Result<i64, Self::Error>;
    fn auto_random_incremental_mask(
        &self,
        table: &Self::Table,
        field_type: &Self::FieldType,
    ) -> i64;
    fn ensure_transaction(&self) -> Result<(), Self::Error>;
    fn current_row_id_shard(&self) -> i64;
    fn compose_auto_random_id(
        &self,
        table: &Self::Table,
        field_type: &Self::FieldType,
        shard: i64,
        incremental: i64,
    ) -> i64;
    fn rebase_auto_random(
        &self,
        context: &Self::Context,
        table: &Self::Table,
        incremental: i64,
    ) -> Result<(), Self::Error>;
    fn alloc_implicit_row_id(
        &self,
        context: &Self::Context,
        table: &Self::Table,
    ) -> Result<i64, Self::Error>;
    fn implicit_row_id_mask(&self, table: &Self::Table) -> i64;
    fn rebase_implicit_row_id(
        &self,
        context: &Self::Context,
        table: &Self::Table,
        incremental: i64,
    ) -> Result<(), Self::Error>;
    fn set_statement_insert_id(&self, id: u64);
    fn set_last_insert_id(&self, id: u64);

    fn transaction(&self) -> Result<Self::Transaction, Self::Error>;
    fn set_top_sql_option(&self, transaction: &Self::Transaction);
    fn foreign_key_check_rows(
        &self,
        checker: &Self::ForeignKeyCheck,
        context: &Self::Context,
        transaction: &Self::Transaction,
        rows: &[ToBeCheckedRow<Self::Datum, Self::Table, Self::Error>],
    ) -> Result<(), Self::Error>;
    fn get_keys_need_check(
        &self,
        table: &Self::Table,
        rows: &[Vec<Self::Datum>],
    ) -> Result<Vec<ToBeCheckedRow<Self::Datum, Self::Table, Self::Error>>, Self::Error>;
    fn prefetch_unique_indices(
        &self,
        context: &Self::Context,
        transaction: &Self::Transaction,
        rows: &[ToBeCheckedRow<Self::Datum, Self::Table, Self::Error>],
    ) -> Result<(), Self::Error>;
    fn transaction_get(
        &self,
        context: &Self::Context,
        transaction: &Self::Transaction,
        key: &[u8],
    ) -> Result<(), Self::Error>;
    fn is_not_found(&self, error: &Self::Error) -> bool;
    fn decode_row_handle(&self, key: &[u8]) -> Result<Self::Handle, Self::Error>;
    fn fetch_duplicated_handle(
        &self,
        context: &Self::Context,
        key: &[u8],
        transaction: &Self::Transaction,
    ) -> Result<Option<Self::Handle>, Self::Error>;
    fn is_temporary_index_key(&self, key: &[u8]) -> bool;
    fn temporary_index_to_index_key(&self, key: &mut Vec<u8>);
    fn add_unchanged_key_for_lock(&self, key: &[u8]);
    fn add_copied_rows(&self, count: u64);
    fn add_affected_rows(&self, count: u64);
    fn add_deleted_rows(&self, count: u64);
    fn old_row(
        &self,
        context: &Self::Context,
        transaction: &Self::Transaction,
        table: &Self::Table,
        handle: &Self::Handle,
        generated_expressions: &[Self::Expression],
    ) -> Result<Vec<Self::Datum>, Self::Error>;
    fn log_old_row_failure(&self, handle: &Self::Handle, inserted_row: &[Self::Datum]);
    fn add_unchanged_keys_for_row(
        &self,
        table: &Self::Table,
        handle: &Self::Handle,
        old_row: &[Self::Datum],
        lock_unique_keys: bool,
    ) -> Result<(), Self::Error>;
    fn remove_record(
        &self,
        transaction: &Self::Transaction,
        table: &Self::Table,
        handle: &Self::Handle,
        old_row: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    fn foreign_keys_on_remove(
        &self,
        old_row: &[Self::Datum],
        checks: &[Self::ForeignKeyCheck],
        cascades: &[Self::ForeignKeyCascade],
        ignore_error: bool,
    ) -> Result<(), Self::Error>;
    fn add_table_record(
        &self,
        context: &Self::Context,
        transaction: &Self::Transaction,
        table: &Self::Table,
        row: &[Self::Datum],
        reserve_auto_id_count: usize,
        duplicate_check: DupKeyCheckMode,
    ) -> Result<(), Self::Error>;
    fn foreign_key_insert_row(
        &self,
        checker: &Self::ForeignKeyCheck,
        row: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    fn increment_ttl_insert_rows(&self);

    fn runtime_stats(&self) -> Option<Self::BasicRuntimeStats>;
    fn new_snapshot_runtime_stats(&self) -> Self::SnapshotRuntimeStats;
    fn new_allocator_runtime_stats(&self) -> Self::AllocatorRuntimeStats;
    fn basic_runtime_time(&self, stats: &Self::BasicRuntimeStats) -> Duration;
    fn snapshot_stats_string(&self, stats: &Self::SnapshotRuntimeStats) -> String;
    fn allocator_stats_string(&self, stats: &Self::AllocatorRuntimeStats) -> String;
    fn clone_snapshot_stats(
        &self,
        stats: &Self::SnapshotRuntimeStats,
    ) -> Self::SnapshotRuntimeStats;
    fn merge_snapshot_stats(
        &self,
        destination: &mut Self::SnapshotRuntimeStats,
        source: &Self::SnapshotRuntimeStats,
    );
    fn clone_allocator_stats(
        &self,
        stats: &Self::AllocatorRuntimeStats,
    ) -> Self::AllocatorRuntimeStats;
    fn merge_allocator_stats(
        &self,
        destination: &mut Self::AllocatorRuntimeStats,
        source: &Self::AllocatorRuntimeStats,
    );
    fn format_duration(&self, duration: Duration) -> String;
    fn insert_runtime_stats_type(&self) -> i32;
    fn start_trace(&self, context: &Self::Context, name: &str) -> Self::TraceGuard;
}

/// 列默认值缓存项（有效标志 + Datum）。
pub struct defaultVal<D> {
    pub val: D,
    pub valid: bool,
}

/// INSERT VALUES 共享状态：目标列、求值缓冲、自增提示与运行时统计。
pub struct InsertValues<B: InsertBackend> {
    pub backend: Arc<B>,
    pub rowCount: u64,
    pub curBatchCnt: u64,
    pub maxRowsInBatch: u64,
    pub lastInsertID: u64,
    pub recordRUV2RowsColMultiply: bool,
    pub ruv2RecordedRowsColMultiply: i64,
    pub SelectExec: Option<B::Executor>,
    pub Table: B::Table,
    pub Columns: Vec<B::ColumnName>,
    pub Lists: Vec<Vec<B::Expression>>,
    pub GenExprs: Vec<B::Expression>,
    pub insertColumns: Vec<B::Column>,
    pub colDefaultVals: Option<Vec<defaultVal<B::Datum>>>,
    pub evalBuffer: Option<B::MutRow>,
    pub evalBufferTypes: Vec<B::FieldType>,
    pub allAssignmentsAreConstant: bool,
    pub hasRefCols: bool,
    pub hasExtraHandle: bool,
    pub lazyFillAutoID: bool,
    pub memTracker: B::MemoryTracker,
    pub rowLen: usize,
    pub stats: Option<InsertRuntimeStat<B>>,
    pub fkChecks: Vec<B::ForeignKeyCheck>,
    pub fkCascades: Vec<B::ForeignKeyCascade>,
    pub ignoreErr: bool,
}

/// INSERT 公共接口：访问 InsertValues 并执行一批行。
pub trait insertCommon<B: InsertBackend> {
    fn insertCommon(&mut self) -> &mut InsertValues<B>;
    fn exec(&mut self, context: &B::Context, rows: &[Vec<B::Datum>]) -> Result<(), B::Error>;
}

impl<B: InsertBackend> insertCommon<B> for InsertValues<B> {
    fn insertCommon(&mut self) -> &mut InsertValues<B> {
        self
    }

    fn exec(&mut self, _context: &B::Context, _rows: &[Vec<B::Datum>]) -> Result<(), B::Error> {
        panic!("derived should overload exec function")
    }
}

impl<B: InsertBackend> InsertValues<B> {
    /// 行数×列数，用于 RU/指标累计。
    pub fn rowsColMultiply(&self) -> i64 {
        let column_count = self.insertColumns.len();
        if self.rowCount == 0 || column_count == 0 {
            return 0;
        }
        let maximum = i64::MAX as u64;
        if self.rowCount > maximum / column_count as u64 {
            return i64::MAX;
        }
        (self.rowCount * column_count as u64) as i64
    }

    /// 将行×列乘积记入 RU v2 指标。
    pub fn recordRowsColMultiply2RUV2Metrics(&mut self) {
        if !self.recordRUV2RowsColMultiply {
            return;
        }
        let current = self.rowsColMultiply();
        let delta = current - self.ruv2RecordedRowsColMultiply;
        if delta <= 0 {
            return;
        }
        self.backend.record_insert_rows_columns_metric(delta);
        self.ruv2RecordedRowsColMultiply = current;
    }

    /// 解析请求列、补全额外 handle 列并做一次性列检查。
    pub fn initInsertColumns(&mut self) -> Result<(), B::Error> {
        let table_columns = self.backend.table_columns(&self.Table);
        let columns = if self.Columns.is_empty() {
            table_columns.clone()
        } else {
            let names: Vec<_> = self
                .Columns
                .iter()
                .map(|column| self.backend.requested_column_lower_name(column))
                .collect();
            let (columns, missing) = self.backend.find_columns(
                &table_columns,
                &names,
                self.backend.table_pk_is_handle(&self.Table),
            );
            if let Some(index) = missing {
                return Err(self.backend.error(format!(
                    "INSERT INTO {}: unknown column {}",
                    self.backend.table_name(&self.Table),
                    self.backend
                        .requested_column_original_name(&self.Columns[index])
                )));
            }
            columns
        };
        for column in &columns {
            if !self.backend.column_is_generated(column) {
                self.insertColumns.push(column.clone());
            }
            if self.backend.column_lower_name(column) == self.backend.extra_handle_name() {
                if !self.backend.allow_write_row_id() {
                    return Err(self.backend.error(
                        "insert, update and replace statements for _tidb_rowid are not supported"
                            .to_owned(),
                    ));
                }
                self.hasExtraHandle = true;
            }
        }
        self.backend.check_columns_once(&columns)
    }

    /// 按列字段类型初始化可变行求值缓冲。
    pub fn initEvalBuffer(&mut self) {
        self.evalBufferTypes = self
            .backend
            .table_columns(&self.Table)
            .iter()
            .map(|column| self.backend.column_field_type(column))
            .collect();
        if self.hasExtraHandle {
            self.evalBufferTypes
                .push(self.backend.longlong_field_type());
        }
        self.evalBuffer = Some(self.backend.new_mut_row(&self.evalBufferTypes));
    }

    /// 惰性分配列默认值缓冲；已分配则返回 false。
    pub fn lazilyInitColDefaultValBuf(&mut self) -> bool {
        if self.colDefaultVals.is_some() {
            return true;
        }
        if self.Lists.len() > 1 {
            self.colDefaultVals = Some(
                self.backend
                    .table_columns(&self.Table)
                    .iter()
                    .map(|_| defaultVal {
                        val: self.backend.null_datum(),
                        valid: false,
                    })
                    .collect(),
            );
            return true;
        }
        false
    }

    /// 按错误种类补全面向用户的 INSERT/LOAD 错误信息。
    pub fn handleErr(
        &self,
        column: Option<&B::Column>,
        value: &B::Datum,
        row_index: usize,
        error: Option<B::Error>,
    ) -> Result<(), B::Error> {
        let Some(mut error) = error else {
            return Ok(());
        };
        // Allocation produced no ID; statement error conversion cannot make it safe.
        if is_terminal_auto_id_error(&error) {
            return Err(error);
        }
        error = if self.backend.in_load_data_statement() {
            completeLoadErr(self.backend.as_ref(), column, row_index, error)
        } else {
            completeInsertErr(self.backend.as_ref(), column, value, row_index, error)
        };
        if column
            .is_some_and(|column| self.backend.column_field_kind(column) == FieldKind::Timestamp)
            && self.backend.error_kind(&error) == InsertErrorKind::TimestampInDstTransition
        {
            let column = column.expect("timestamp column must exist");
            let value_string = self.backend.datum_to_string(value).unwrap_or_default();
            let converted = self.backend.wrong_insert_value_error(
                FieldKind::Timestamp,
                &value_string,
                &self.backend.column_name(column),
                row_index + 1,
            );
            if !self.ignoreErr && self.backend.strict_sql_mode() {
                return Err(converted);
            }
            self.backend.append_warning(converted);
            return Ok(());
        }
        self.backend.handle_statement_error(error)
    }

    /// 对一行各列求值（含引用列与类型转换）。
    pub fn evalRow(
        &mut self,
        context: &B::Context,
        list: &[B::Expression],
        row_index: usize,
    ) -> Result<Vec<B::Datum>, B::Error> {
        let mut row = vec![self.backend.null_datum(); self.evalBufferTypes.len()];
        let mut has_value = vec![false; row.len()];
        if self.hasRefCols {
            self.setValueForRefColumn(&mut row, &mut has_value)?;
        }
        let mut eval_buffer = self
            .evalBuffer
            .take()
            .expect("insert evaluation buffer must be initialized");
        let evaluation = (|| {
            self.backend.mut_row_set_datums(&mut eval_buffer, &row);
            let mut warning_count = self.backend.warning_count();
            for (index, expression) in list.iter().enumerate() {
                let value = self.backend.eval_expression(expression, &eval_buffer)?;
                let casted = match self.backend.cast_value(&value, &self.insertColumns[index]) {
                    Ok(value) => value,
                    Err(error) => {
                        self.handleErr(
                            Some(&self.insertColumns[index]),
                            &value,
                            row_index,
                            Some(error),
                        )?;
                        self.backend.null_datum()
                    }
                };
                warning_count = self.rewrite_warnings(
                    warning_count,
                    &self.insertColumns[index],
                    &value,
                    row_index,
                    false,
                );
                let offset = self.backend.column_offset(&self.insertColumns[index]);
                row[offset] = casted.clone();
                has_value[offset] = true;
                self.backend
                    .mut_row_set_datum(&mut eval_buffer, offset, &casted);
            }
            Ok(())
        })();
        self.evalBuffer = Some(eval_buffer);
        evaluation?;
        self.fillRow(context, row, has_value, row_index)
    }

    /// 常量赋值快速路径：跳过完整表达式求值。
    pub fn fastEvalRow(
        &mut self,
        context: &B::Context,
        list: &[B::Expression],
        row_index: usize,
    ) -> Result<Vec<B::Datum>, B::Error> {
        let mut row = vec![self.backend.null_datum(); self.evalBufferTypes.len()];
        let mut has_value = vec![false; row.len()];
        let mut warning_count = self.backend.warning_count();
        for (index, expression) in list.iter().enumerate() {
            let value = match self.backend.eval_constant(expression) {
                Ok(value) => value,
                Err(error) => {
                    let value = self.backend.null_datum();
                    self.handleErr(
                        Some(&self.insertColumns[index]),
                        &value,
                        row_index,
                        Some(error),
                    )?;
                    value
                }
            };
            let casted = match self.backend.cast_value(&value, &self.insertColumns[index]) {
                Ok(value) => value,
                Err(error) => {
                    self.handleErr(
                        Some(&self.insertColumns[index]),
                        &value,
                        row_index,
                        Some(error),
                    )?;
                    self.backend.null_datum()
                }
            };
            warning_count = self.rewrite_warnings(
                warning_count,
                &self.insertColumns[index],
                &value,
                row_index,
                false,
            );
            let offset = self.backend.column_offset(&self.insertColumns[index]);
            row[offset] = casted;
            has_value[offset] = true;
        }
        self.fillRow(context, row, has_value, row_index)
    }

    /// 为引用列（如 DEFAULT）填入对应值。
    pub fn setValueForRefColumn(
        &mut self,
        row: &mut [B::Datum],
        has_value: &mut [bool],
    ) -> Result<(), B::Error> {
        for (index, column) in self.backend.table_columns(&self.Table).iter().enumerate() {
            match self.getColDefaultValue(index, column) {
                Ok(datum) => {
                    row[index] = datum;
                    if !self.backend.column_is_auto_increment(column) {
                        has_value[self.backend.column_offset(column)] = true;
                    }
                }
                Err(error)
                    if self.backend.error_kind(&error) == InsertErrorKind::NoDefaultValue =>
                {
                    row[index] = self.backend.zero_value(column);
                    has_value[self.backend.column_offset(column)] = false;
                }
                Err(error) => {
                    let datum = self.backend.null_datum();
                    if self
                        .handleErr(Some(column), &datum, 0, Some(error.clone()))
                        .is_err()
                    {
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }

    /// 在需要时提交语句并开启新事务以继续批量插入。
    pub fn doBatchInsert(&self, context: &B::Context) -> Result<(), B::Error> {
        self.backend.statement_commit(context);
        self.backend
            .new_transaction_in_statement(context)
            .map_err(|error| self.backend.batch_insert_error(error))
    }

    /// 组装一行 Datum：求值、填默认值并调整自增等特殊列。
    pub fn getRow(
        &mut self,
        context: &B::Context,
        values: &[B::Datum],
    ) -> Result<Vec<B::Datum>, B::Error> {
        let table_columns = self.backend.table_columns(&self.Table);
        let mut row = vec![self.backend.null_datum(); table_columns.len()];
        let mut has_value = vec![false; table_columns.len()];
        let mut warning_count = self.backend.warning_count();
        for index in 0..self.rowLen {
            let casted = match self
                .backend
                .cast_value(&values[index], &self.insertColumns[index])
            {
                Ok(value) => value,
                Err(error) => {
                    let handled = self.handleErr(
                        Some(&self.insertColumns[index]),
                        &values[index],
                        self.rowCount as usize,
                        Some(error.clone()),
                    );
                    resolve_get_row_cast_error(
                        self.backend.in_load_data_statement(),
                        error,
                        handled,
                    )?;
                    self.backend.null_datum()
                }
            };
            let offset = self.backend.column_offset(&self.insertColumns[index]);
            row[offset] = casted;
            has_value[offset] = true;
            if self.backend.in_load_data_statement() {
                warning_count = self.rewrite_warnings(
                    warning_count,
                    &self.insertColumns[index],
                    &values[index],
                    self.rowCount as usize,
                    true,
                );
            }
        }
        self.fillRow(context, row, has_value, 0)
    }

    /// 获取列默认值（缓存或求值），处理无默认等错误。
    pub fn getColDefaultValue(
        &mut self,
        index: usize,
        column: &B::Column,
    ) -> Result<B::Datum, B::Error> {
        if !self.backend.column_default_is_expression(column)
            && let Some(values) = self.colDefaultVals.as_ref()
            && values[index].valid
        {
            return Ok(values[index].val.clone());
        }
        let default = if self.backend.column_default_is_expression(column)
            && self.backend.column_has_default_expression(column)
        {
            self.backend.eval_default_expression(column)?
        } else {
            self.backend.check_no_default_for_insert(column)?;
            self.backend.column_default_value(column)?
        };
        if self.lazilyInitColDefaultValBuf() && !self.backend.column_default_is_expression(column) {
            let values = self
                .colDefaultVals
                .as_mut()
                .expect("default value buffer must be initialized");
            values[index].val = default.clone();
            values[index].valid = true;
        }
        Ok(default)
    }

    /// 为单列填值：显式值、默认值或生成列逻辑。
    pub fn fillColValue(
        &mut self,
        context: &B::Context,
        mut datum: B::Datum,
        index: usize,
        column: &B::Column,
        has_value: bool,
    ) -> Result<B::Datum, B::Error> {
        if self.backend.column_is_auto_increment(column) {
            if !has_value && self.backend.column_has_no_default(column) {
                let error = self.backend.no_default_value_error(column);
                if self.backend.strict_sql_mode() {
                    return Err(error);
                }
                self.backend.append_warning(error);
            }
            if self.lazyFillAutoID {
                if !has_value {
                    self.backend.datum_set_null(&mut datum);
                }
                return Ok(datum);
            }
            return self.adjustAutoIncrementDatum(context, datum, has_value, column);
        }
        if self
            .backend
            .table_auto_random_column(&self.Table, self.backend.column_id(column))
        {
            return self.adjustAutoRandomDatum(context, datum, has_value, column);
        }
        if self.backend.column_id(column) == self.backend.extra_handle_id() && has_value {
            return self.adjustImplicitRowID(context, datum, has_value, column);
        }
        if !has_value {
            return match self.getColDefaultValue(index, column) {
                Ok(value) => Ok(value),
                Err(error) => {
                    self.handleErr(Some(column), &datum, 0, Some(error.clone()))?;
                    Ok(self.backend.null_datum())
                }
            };
        }
        Ok(datum)
    }

    /// 填充整行：遍历列、处理自增/自随机/隐式 row id。
    pub fn fillRow(
        &mut self,
        context: &B::Context,
        mut row: Vec<B::Datum>,
        mut has_value: Vec<bool>,
        row_index: usize,
    ) -> Result<Vec<B::Datum>, B::Error> {
        let mut table_columns = self.backend.table_columns(&self.Table);
        if self.hasExtraHandle {
            table_columns.push(self.backend.new_extra_handle_column(table_columns.len()));
            if has_value.len() < table_columns.len() {
                has_value.push(false);
                row.push(self.backend.null_datum());
            }
        }
        let load_row_count = if self.backend.in_load_data_statement() {
            self.rowCount
        } else {
            0
        };
        let mut generated_columns = Vec::new();
        for (index, column) in table_columns.iter().enumerate() {
            if self.backend.column_is_generated(column) {
                generated_columns.push(column.clone());
                continue;
            }
            row[index] =
                self.fillColValue(context, row[index].clone(), index, column, has_value[index])?;
            if !self.lazyFillAutoID || !self.backend.column_is_auto_increment(column) {
                self.backend
                    .handle_bad_null(column, &mut row[index], load_row_count)?;
            }
        }
        if self
            .backend
            .table_has_exchange_without_partition(&self.Table)
        {
            self.backend
                .check_exchange_partition_row(&self.Table, &row)?;
        }
        if generated_columns.is_empty() {
            return Ok(row);
        }
        let mut mutable_row = self.backend.new_mut_row(&self.evalBufferTypes);
        self.backend.mut_row_set_datums(&mut mutable_row, &row);
        let mut warning_count = self.backend.warning_count();
        for (index, column) in generated_columns.iter().enumerate() {
            let value = match self
                .backend
                .eval_expression(&self.GenExprs[index], &mutable_row)
            {
                Ok(value) => value,
                Err(error) if self.backend.column_is_array(column) => {
                    return Err(completeError(
                        self.backend.as_ref(),
                        &self.Table,
                        self.backend.column_offset(column),
                        row_index,
                        error,
                    ));
                }
                Err(error) => {
                    self.backend.handle_truncate(error)?;
                    self.backend.null_datum()
                }
            };
            let offset = self.backend.column_offset(column);
            row[offset] = match self.backend.cast_value(&value, column) {
                Ok(value) => value,
                Err(error) => {
                    self.handleErr(Some(column), &value, row_index, Some(error))?;
                    self.backend.null_datum()
                }
            };
            warning_count = self.rewrite_warnings(warning_count, column, &value, row_index, false);
            self.backend
                .handle_bad_null(column, &mut row[offset], load_row_count)?;
            self.backend
                .mut_row_set_datum(&mut mutable_row, offset, &row[offset]);
        }
        Ok(row)
    }

    /// 判断自增列当前值是否视为“空”（需分配新 ID）。
    pub fn isAutoNull(&self, _context: &B::Context, datum: &B::Datum, column: &B::Column) -> bool {
        let record_id = if self.backend.datum_is_null(datum) {
            0
        } else {
            match getAutoRecordID(
                self.backend.as_ref(),
                datum,
                &self.backend.column_field_type(column),
                true,
            ) {
                Ok(id) => id,
                Err(_) => return false,
            }
        };
        record_id == 0
            && (self.backend.datum_is_null(datum) || !self.backend.no_auto_value_on_zero())
    }

    /// 惰性调整自增 Datum（批分配/ rebase）。
    pub fn lazyAdjustAutoIncrementDatum(
        &mut self,
        context: &B::Context,
        mut rows: Vec<Vec<B::Datum>>,
    ) -> Result<Vec<Vec<B::Datum>>, B::Error> {
        if !self.lazyFillAutoID {
            return Ok(rows);
        }
        let Some((column, column_index)) =
            findAutoIncrementColumn(self.backend.as_ref(), &self.Table)
        else {
            return Ok(rows);
        };
        let row_count = rows.len();
        let mut processed = 0;
        while processed < row_count {
            let auto_datum = rows[processed][column_index].clone();
            let record_id = if self.backend.datum_is_null(&auto_datum) {
                0
            } else {
                getAutoRecordID(
                    self.backend.as_ref(),
                    &auto_datum,
                    &self.backend.column_field_type(&column),
                    true,
                )?
            };
            if record_id != 0 {
                self.backend
                    .rebase_auto_increment(context, &self.Table, record_id)?;
                self.backend.set_statement_insert_id(record_id as u64);
                self.backend.add_retry_auto_increment_id(record_id);
                processed += 1;
                continue;
            }
            if self.backend.datum_is_null(&auto_datum) || !self.backend.no_auto_value_on_zero() {
                while self.backend.retrying() && processed < row_count {
                    let Some(next_id) = self.backend.next_retry_auto_increment_id() else {
                        break;
                    };
                    setDatumAutoIDAndCast(
                        self.backend.as_ref(),
                        &mut rows[processed][column_index],
                        next_id,
                        &column,
                    )?;
                    processed += 1;
                }
                if processed == row_count {
                    return Ok(rows);
                }
                let start = processed;
                let mut count = 1;
                while processed + 1 < row_count
                    && self.isAutoNull(context, &rows[processed + 1][column_index], &column)
                {
                    processed += 1;
                    count += 1;
                }
                let allocation =
                    self.backend
                        .alloc_batch_auto_increment(context, &self.Table, count);
                let (minimum, increment) = match allocation {
                    Ok(value) => value,
                    Err(error) => {
                        self.handleErr(Some(&column), &auto_datum, count, Some(error.clone()))?;
                        return Err(error);
                    }
                };
                if self.lastInsertID == 0 {
                    self.lastInsertID = minimum as u64;
                }
                for index in 0..count {
                    let id = minimum.wrapping_add((index as i64).wrapping_mul(increment));
                    setDatumAutoIDAndCast(
                        self.backend.as_ref(),
                        &mut rows[start + index][column_index],
                        id,
                        &column,
                    )?;
                    self.backend.add_retry_auto_increment_id(id);
                }
                processed += 1;
                continue;
            }
            setDatumAutoIDAndCast(
                self.backend.as_ref(),
                &mut rows[processed][column_index],
                record_id,
                &column,
            )?;
            self.backend.add_retry_auto_increment_id(record_id);
            processed += 1;
        }
        Ok(rows)
    }

    /// 立即调整自增 Datum 并更新 last_insert_id。
    pub fn adjustAutoIncrementDatum(
        &mut self,
        context: &B::Context,
        mut datum: B::Datum,
        has_value: bool,
        column: &B::Column,
    ) -> Result<B::Datum, B::Error> {
        if self.backend.retrying()
            && let Some(id) = self.backend.next_retry_auto_increment_id()
        {
            setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, id, column)?;
            return Ok(datum);
        }
        if !has_value {
            self.backend.datum_set_null(&mut datum);
        }
        let mut record_id = if self.backend.datum_is_null(&datum) {
            0
        } else {
            getAutoRecordID(
                self.backend.as_ref(),
                &datum,
                &self.backend.column_field_type(column),
                true,
            )?
        };
        if record_id != 0 {
            self.backend
                .rebase_auto_increment(context, &self.Table, record_id)?;
            self.backend.set_statement_insert_id(record_id as u64);
            self.backend.add_retry_auto_increment_id(record_id);
            return Ok(datum);
        }
        if self.backend.datum_is_null(&datum) || !self.backend.no_auto_value_on_zero() {
            match self.backend.alloc_auto_increment(context, &self.Table) {
                Ok(id) => record_id = id,
                Err(error) => {
                    self.handleErr(Some(column), &datum, 0, Some(error.clone()))?;
                    return Err(error);
                }
            }
            if self.lastInsertID == 0 {
                self.lastInsertID = record_id as u64;
            }
        }
        setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, record_id, column)?;
        self.backend.add_retry_auto_increment_id(record_id);
        Ok(datum)
    }

    /// 调整 auto_random 列值（显式插入或分配）。
    pub fn adjustAutoRandomDatum(
        &mut self,
        context: &B::Context,
        mut datum: B::Datum,
        has_value: bool,
        column: &B::Column,
    ) -> Result<B::Datum, B::Error> {
        if self.backend.retrying()
            && let Some(id) = self.backend.next_retry_auto_random_id()
        {
            setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, id, column)?;
            return Ok(datum);
        }
        if !has_value {
            self.backend.datum_set_null(&mut datum);
        }
        let mut record_id = if self.backend.datum_is_null(&datum) {
            0
        } else {
            getAutoRecordID(
                self.backend.as_ref(),
                &datum,
                &self.backend.column_field_type(column),
                true,
            )?
        };
        if record_id != 0 {
            if !self.backend.allow_auto_random_explicit_insert() {
                return Err(self.backend.invalid_auto_random_error());
            }
            self.rebaseAutoRandomID(context, record_id, &self.backend.column_field_type(column))?;
            self.backend.set_statement_insert_id(record_id as u64);
            setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, record_id, column)?;
            self.backend.add_retry_auto_random_id(record_id);
            return Ok(datum);
        }
        if self.backend.datum_is_null(&datum) || !self.backend.no_auto_value_on_zero() {
            record_id = self.allocAutoRandomID(context, &self.backend.column_field_type(column))?;
            if self.lastInsertID == 0 {
                self.lastInsertID = record_id as u64;
            }
        }
        setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, record_id, column)?;
        self.backend.add_retry_auto_random_id(record_id);
        Ok(datum)
    }

    /// 分配一个 auto_random ID。
    pub fn allocAutoRandomID(
        &self,
        context: &B::Context,
        field_type: &B::FieldType,
    ) -> Result<i64, B::Error> {
        let incremental = self
            .backend
            .alloc_auto_random_incremental(context, &self.Table)?;
        let mask = self
            .backend
            .auto_random_incremental_mask(&self.Table, field_type);
        if mask & incremental != incremental {
            return Err(self.backend.auto_random_read_failed());
        }
        self.backend.ensure_transaction()?;
        Ok(self.backend.compose_auto_random_id(
            &self.Table,
            field_type,
            self.backend.current_row_id_shard(),
            incremental,
        ))
    }

    /// 按显式值 rebase auto_random 分配器。
    pub fn rebaseAutoRandomID(
        &self,
        context: &B::Context,
        record_id: i64,
        field_type: &B::FieldType,
    ) -> Result<(), B::Error> {
        if record_id < 0 {
            return Ok(());
        }
        let incremental = self
            .backend
            .auto_random_incremental_mask(&self.Table, field_type)
            & record_id;
        self.backend
            .rebase_auto_random(context, &self.Table, incremental)
    }

    /// 调整隐式 _tidb_rowid。
    pub fn adjustImplicitRowID(
        &self,
        context: &B::Context,
        mut datum: B::Datum,
        has_value: bool,
        column: &B::Column,
    ) -> Result<B::Datum, B::Error> {
        if !has_value {
            self.backend.datum_set_null(&mut datum);
        }
        let mut record_id = if self.backend.datum_is_null(&datum) {
            0
        } else {
            self.backend.datum_get_int64(&datum)
        };
        if record_id != 0 {
            if !self.backend.allow_write_row_id() {
                return Err(self.backend.error(
                    "insert, update and replace statements for _tidb_rowid are not supported"
                        .to_owned(),
                ));
            }
            self.rebaseImplicitRowID(context, record_id)?;
            self.backend.datum_set_int64(&mut datum, record_id);
            return Ok(datum);
        }
        if self.backend.datum_is_null(&datum) || !self.backend.no_auto_value_on_zero() {
            self.backend
                .ensure_transaction()
                .map_err(|error| self.backend.trace_error(error))?;
            record_id = self.backend.alloc_implicit_row_id(context, &self.Table)?;
        }
        setDatumAutoIDAndCast(self.backend.as_ref(), &mut datum, record_id, column)?;
        Ok(datum)
    }

    /// 按显式值 rebase 隐式 row id 分配器。
    pub fn rebaseImplicitRowID(
        &self,
        context: &B::Context,
        record_id: i64,
    ) -> Result<(), B::Error> {
        if record_id < 0 {
            return Ok(());
        }
        let incremental = self.backend.implicit_row_id_mask(&self.Table) & record_id;
        self.backend
            .rebase_implicit_row_id(context, &self.Table, incremental)
    }

    /// 将错误作为语句警告追加。
    pub fn handleWarning(&self, error: B::Error) {
        self.backend.append_warning(error);
    }

    /// 若开启则初始化快照/分配器运行时统计并返回 true。
    pub fn collectRuntimeStatsEnabled(&mut self) -> bool {
        let Some(basic) = self.backend.runtime_stats() else {
            return false;
        };
        if self.stats.is_none() {
            self.stats = Some(InsertRuntimeStat {
                backend: Arc::clone(&self.backend),
                BasicRuntimeStats: Some(basic),
                SnapshotRuntimeStats: Some(self.backend.new_snapshot_runtime_stats()),
                AllocatorRuntimeStats: Some(self.backend.new_allocator_runtime_stats()),
                CheckInsertTime: Duration::ZERO,
                Prefetch: Duration::ZERO,
                FKCheckTime: Duration::ZERO,
            });
        }
        true
    }

    /// 处理重复键：可选加锁或返回预构造冲突错误。
    pub fn handleDuplicateKey(
        &mut self,
        context: &B::Context,
        transaction: &B::Transaction,
        unique_key: &DuplicateKey<B::Error>,
        replace: bool,
        row: &ToBeCheckedRow<B::Datum, B::Table, B::Error>,
    ) -> Result<bool, B::Error> {
        if !replace {
            self.backend
                .append_warning(unique_key.duplicate_error.clone());
            if self.backend.pessimistic_transaction() && self.backend.lock_unchanged_keys() {
                self.backend.add_unchanged_key_for_lock(&unique_key.new_key);
            }
            return Ok(true);
        }
        let Some(handle) =
            self.backend
                .fetch_duplicated_handle(context, &unique_key.new_key, transaction)?
        else {
            return Ok(false);
        };
        self.removeRow(context, transaction, &handle, row, true)
    }

    /// 批量检查唯一键后插入；冲突时回调处理。
    pub fn batchCheckAndInsert<F>(
        &mut self,
        context: &B::Context,
        rows: &[Vec<B::Datum>],
        mut add_record: F,
        replace: bool,
    ) -> Result<(), B::Error>
    where
        F: FnMut(&B::Context, &[B::Datum], DupKeyCheckMode) -> Result<(), B::Error>,
    {
        let _trace = self
            .backend
            .start_trace(context, "InsertValues.batchCheckAndInsert");
        let start = Instant::now();
        let mut checked_rows = self.backend.get_keys_need_check(&self.Table, rows)?;
        let transaction = self.backend.transaction()?;
        self.backend.set_top_sql_option(&transaction);
        for checker in &self.fkChecks {
            self.backend
                .foreign_key_check_rows(checker, context, &transaction, &checked_rows)?;
        }
        let prefetch_start = Instant::now();
        if !self.backend.table_is_temporary(&self.Table) {
            self.backend
                .prefetch_unique_indices(context, &transaction, &checked_rows)?;
        }
        if let Some(stats) = self.stats.as_mut() {
            stats.FKCheckTime += prefetch_start.duration_since(start);
            stats.Prefetch += prefetch_start.elapsed();
        }

        for (index, checked) in checked_rows.iter_mut().enumerate() {
            if checked.ignored {
                continue;
            }
            if let Some(handle_key) = checked.handle_key.as_ref() {
                match self
                    .backend
                    .transaction_get(context, &transaction, &handle_key.new_key)
                {
                    Ok(()) if !replace => {
                        self.backend
                            .append_warning(handle_key.duplicate_error.clone());
                        if self.backend.pessimistic_transaction()
                            && self.backend.lock_unchanged_keys()
                        {
                            self.backend.add_unchanged_key_for_lock(&handle_key.new_key);
                        }
                        continue;
                    }
                    Ok(()) => {
                        let handle = self.backend.decode_row_handle(&handle_key.new_key)?;
                        if self.removeRow(context, &transaction, &handle, checked, false)? {
                            self.backend.add_copied_rows(1);
                            continue;
                        }
                    }
                    Err(error) if self.backend.is_not_found(&error) => {}
                    Err(error) => return Err(error),
                }
            }

            let mut row_inserted = false;
            for unique_key_index in 0..checked.unique_keys.len() {
                let mut unique_key = checked.unique_keys[unique_key_index].clone();
                let mut found =
                    match self
                        .backend
                        .transaction_get(context, &transaction, &unique_key.new_key)
                    {
                        Ok(()) => true,
                        Err(error) if self.backend.is_not_found(&error) => false,
                        Err(error) => return Err(error),
                    };
                if !found && self.backend.is_temporary_index_key(&unique_key.new_key) {
                    self.backend
                        .temporary_index_to_index_key(&mut unique_key.new_key);
                    checked.unique_keys[unique_key_index].new_key = unique_key.new_key.clone();
                    found = match self.backend.transaction_get(
                        context,
                        &transaction,
                        &unique_key.new_key,
                    ) {
                        Ok(()) => true,
                        Err(error) if self.backend.is_not_found(&error) => false,
                        Err(error) => return Err(error),
                    };
                }
                if found {
                    row_inserted = self.handleDuplicateKey(
                        context,
                        &transaction,
                        &unique_key,
                        replace,
                        checked,
                    )?;
                    if row_inserted {
                        break;
                    }
                }
            }
            if row_inserted {
                continue;
            }
            self.backend.add_copied_rows(1);
            if let Err(error) = add_record(context, &rows[index], DupKeyCheckMode::Skip) {
                if self.backend.error_kind(&error) == InsertErrorKind::CheckConstraintViolated {
                    if !self.backend.in_load_data_statement() {
                        self.backend.append_warning(error);
                    }
                    continue;
                }
                return Err(error);
            }
        }
        if let Some(stats) = self.stats.as_mut() {
            stats.CheckInsertTime += start.elapsed();
        }
        Ok(())
    }

    /// 删除冲突旧行并更新外键/影响行计数。
    pub fn removeRow(
        &mut self,
        context: &B::Context,
        transaction: &B::Transaction,
        handle: &B::Handle,
        row: &ToBeCheckedRow<B::Datum, B::Table, B::Error>,
        in_replace: bool,
    ) -> Result<bool, B::Error> {
        let old_row =
            match self
                .backend
                .old_row(context, transaction, &row.table, handle, &self.GenExprs)
            {
                Ok(row) => row,
                Err(error) => {
                    self.backend.log_old_row_failure(handle, &row.row);
                    if self.backend.is_not_found(&error) {
                        return Err(self.backend.old_row_not_found_error(handle));
                    }
                    return Err(error);
                }
            };
        if self.equalDatumsAsBinary(&old_row, &row.row)? {
            if in_replace {
                self.backend.add_affected_rows(1);
            }
            self.backend.add_unchanged_keys_for_row(
                &row.table,
                handle,
                &old_row,
                self.backend.lock_unchanged_keys(),
            )?;
            return Ok(true);
        }
        self.backend
            .remove_record(transaction, &row.table, handle, &old_row)?;
        self.backend.foreign_keys_on_remove(
            &old_row,
            &self.fkChecks,
            &self.fkCascades,
            self.ignoreErr,
        )?;
        if in_replace {
            self.backend.add_affected_rows(1);
        } else {
            self.backend.add_deleted_rows(1);
        }
        Ok(false)
    }

    /// 以 binary 校对规则比较两组 Datum 是否全等。
    pub fn equalDatumsAsBinary(
        &self,
        left: &[B::Datum],
        right: &[B::Datum],
    ) -> Result<bool, B::Error> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if self
                .backend
                .datum_compare_binary(left, right)
                .map_err(|error| self.backend.trace_error(error))?
                != 0
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// 写入一行记录（无 auto id 提示）。
    pub fn addRecord(
        &mut self,
        context: &B::Context,
        row: &[B::Datum],
        duplicate_check: DupKeyCheckMode,
    ) -> Result<(), B::Error> {
        self.addRecordWithAutoIDHint(context, row, 0, duplicate_check)
    }

    /// 带批大小提示写入记录，便于预分配自增 ID。
    pub fn addRecordWithAutoIDHint(
        &mut self,
        context: &B::Context,
        row: &[B::Datum],
        reserve_auto_id_count: usize,
        duplicate_check: DupKeyCheckMode,
    ) -> Result<(), B::Error> {
        let transaction = self.backend.transaction()?;
        self.backend.add_table_record(
            context,
            &transaction,
            &self.Table,
            row,
            reserve_auto_id_count,
            duplicate_check,
        )?;
        self.backend.add_affected_rows(1);
        if self.lastInsertID != 0 {
            self.backend.set_last_insert_id(self.lastInsertID);
        }
        if duplicate_check != DupKeyCheckMode::Skip {
            for checker in &self.fkChecks {
                self.backend.foreign_key_insert_row(checker, row)?;
            }
        }
        if self.backend.table_has_ttl(&self.Table) {
            self.backend.increment_ttl_insert_rows();
        }
        Ok(())
    }

    /// 将截断类警告改写为带列名/行号的完整消息。
    fn rewrite_warnings(
        &self,
        warning_count: usize,
        column: &B::Column,
        value: &B::Datum,
        row_index: usize,
        load: bool,
    ) -> usize {
        let warnings = self.backend.take_warnings_since(warning_count);
        let count = warnings.len();
        let warnings = warnings
            .into_iter()
            .map(|error| {
                if load {
                    completeLoadErr(self.backend.as_ref(), Some(column), row_index, error)
                } else {
                    completeInsertErr(self.backend.as_ref(), Some(column), value, row_index, error)
                }
            })
            .collect();
        self.backend.append_warnings(warnings);
        warning_count + count
    }
}

/// VALUES 插入主路径：求值填行、可选批事务、逐行/批量写入。
pub fn insertRows<B: InsertBackend, C: insertCommon<B>>(
    context: &B::Context,
    base: &mut C,
) -> Result<(), B::Error> {
    let (lists, constant, batch_insert, batch_size, tracker) = {
        let insert = base.insertCommon();
        insert.lazyFillAutoID = true;
        (
            insert.Lists.clone(),
            insert.allAssignmentsAreConstant,
            insert.backend.batch_insert_enabled()
                && !insert.backend.in_transaction()
                && insert.backend.batch_dml_enabled()
                && insert.backend.dml_batch_size() > 0,
            insert.backend.dml_batch_size(),
            insert.memTracker.clone(),
        )
    };
    let mut rows = Vec::with_capacity(lists.len());
    let mut memory_usage = 0;
    for (index, list) in lists.iter().enumerate() {
        let row = {
            let insert = base.insertCommon();
            insert.rowCount += 1;
            if constant {
                insert.fastEvalRow(context, list, index)?
            } else {
                insert.evalRow(context, list, index)?
            }
        };
        rows.push(row);
        let should_flush = {
            let insert = base.insertCommon();
            batch_insert && insert.rowCount % batch_size as u64 == 0
        };
        if should_flush {
            let backend = Arc::clone(&base.insertCommon().backend);
            memory_usage = backend.estimated_rows_memory(&rows[0], rows.len());
            backend.memory_consume(&tracker, memory_usage);
            rows = base
                .insertCommon()
                .lazyAdjustAutoIncrementDatum(context, rows)?;
            base.exec(context, &rows)?;
            {
                let insert = base.insertCommon();
                insert.recordRowsColMultiply2RUV2Metrics();
                backend.memory_consume(&tracker, -memory_usage);
                memory_usage = 0;
                insert.doBatchInsert(context)?;
            }
            rows.clear();
        }
    }
    if !rows.is_empty() {
        let backend = Arc::clone(&base.insertCommon().backend);
        memory_usage = backend.estimated_rows_memory(&rows[0], rows.len());
        backend.memory_consume(&tracker, memory_usage);
    }
    rows = base
        .insertCommon()
        .lazyAdjustAutoIncrementDatum(context, rows)?;
    base.exec(context, &rows)?;
    let insert = base.insertCommon();
    insert.recordRowsColMultiply2RUV2Metrics();
    insert.backend.memory_consume(&tracker, -memory_usage);
    Ok(())
}

/// INSERT…SELECT 路径：从子执行器取 chunk 并写入。
pub fn insertRowsFromSelect<B: InsertBackend, C: insertCommon<B>>(
    context: &B::Context,
    base: &mut C,
) -> Result<(), B::Error> {
    let (executor, fields, mut chunk, capacity, tracker, batch_insert, batch_size, row_len) = {
        let insert = base.insertCommon();
        let executor = insert.backend.select_executor();
        let fields = insert.backend.executor_field_types(&executor);
        let chunk = insert.backend.new_executor_chunk(&executor);
        let capacity = insert.backend.chunk_capacity(&chunk);
        (
            executor,
            fields,
            chunk,
            capacity,
            insert.memTracker.clone(),
            insert.backend.batch_insert_enabled()
                && !insert.backend.in_transaction()
                && insert.backend.batch_dml_enabled()
                && insert.backend.dml_batch_size() > 0,
            insert.backend.dml_batch_size(),
            insert.rowLen,
        )
    };
    let backend = Arc::clone(&base.insertCommon().backend);
    backend.invalidate_transaction_write_throughput_sli();
    let mut rows = Vec::with_capacity(capacity);
    let mut extra_columns = Vec::with_capacity(capacity);
    loop {
        let selected_rows = backend.executor_next_rows(context, &executor, &mut chunk, &fields)?;
        if selected_rows.is_empty() {
            break;
        }
        let chunk_memory = backend.chunk_memory_usage(&chunk);
        backend.memory_consume(&tracker, chunk_memory);
        let mut total_delta = 0;
        for selected_row in selected_rows {
            let row = {
                let insert = base.insertCommon();
                insert.rowCount += 1;
                insert.getRow(context, &selected_row)?
            };
            extra_columns.push(selected_row[row_len..].to_vec());
            rows.push(row);
            let flush = batch_insert && base.insertCommon().rowCount % batch_size as u64 == 0;
            if flush {
                let row_memory = backend.estimated_rows_memory(&rows[0], rows.len());
                let extra_memory =
                    backend.estimated_rows_memory(&extra_columns[0], extra_columns.len());
                total_delta += row_memory + extra_memory;
                backend.set_current_insert_batch_extra_columns(&extra_columns);
                base.exec(context, &rows)?;
                base.insertCommon().recordRowsColMultiply2RUV2Metrics();
                rows.clear();
                extra_columns.clear();
                total_delta -= row_memory + extra_memory;
                base.insertCommon().doBatchInsert(context)?;
            }
        }
        backend.memory_consume(&tracker, total_delta);
        let (row_memory, extra_memory) = if rows.is_empty() {
            (0, 0)
        } else {
            let row_memory = backend.estimated_rows_memory(&rows[0], rows.len());
            let extra_memory =
                backend.estimated_rows_memory(&extra_columns[0], extra_columns.len());
            backend.memory_consume(&tracker, row_memory + extra_memory);
            backend.set_current_insert_batch_extra_columns(&extra_columns);
            (row_memory, extra_memory)
        };
        base.exec(context, &rows)?;
        base.insertCommon().recordRowsColMultiply2RUV2Metrics();
        rows.clear();
        extra_columns.clear();
        backend.memory_consume(&tracker, -row_memory - extra_memory - chunk_memory);
    }
    Ok(())
}

/// 将底层转换错误补全为带列名与一行号的 INSERT 错误。
pub fn completeInsertErr<B: InsertBackend>(
    backend: &B,
    column: Option<&B::Column>,
    value: &B::Datum,
    row_index: usize,
    error: B::Error,
) -> B::Error {
    let column_name = column
        .map(|column| backend.column_name(column))
        .unwrap_or_default();
    let field_kind = column
        .map(|column| backend.column_field_kind(column))
        .unwrap_or(FieldKind::Other);
    match backend.error_kind(&error) {
        InsertErrorKind::DataTooLong => {
            backend.reset_data_too_long_error(&column_name, row_index + 1, error)
        }
        InsertErrorKind::Overflow | InsertErrorKind::WarnDataOutOfRange => {
            backend.data_out_of_range_error(&column_name, row_index + 1)
        }
        InsertErrorKind::Truncated => backend.truncated_error(&column_name, row_index + 1),
        InsertErrorKind::TruncatedWrongValue
            if matches!(
                field_kind,
                FieldKind::Duration | FieldKind::DateTime | FieldKind::Date | FieldKind::Timestamp
            ) =>
        {
            let value = backend
                .datum_to_string(value)
                .unwrap_or_else(|conversion_error| {
                    backend.log_value_conversion_error(&conversion_error, "time truncated error");
                    String::new()
                });
            backend.wrong_insert_value_error(field_kind, &value, &column_name, row_index + 1)
        }
        InsertErrorKind::TruncatedWrongValue | InsertErrorKind::WrongValue => {
            let value = backend
                .datum_to_string(value)
                .unwrap_or_else(|conversion_error| {
                    backend.log_value_conversion_error(
                        &conversion_error,
                        "truncated/wrong value error",
                    );
                    String::new()
                });
            backend.wrong_value_for_field_error(field_kind, &value, &column_name, row_index + 1)
        }
        _ => error,
    }
}

/// 补全 LOAD DATA 错误（DataTooLong 改写为 Truncated）。
pub fn completeLoadErr<B: InsertBackend>(
    backend: &B,
    column: Option<&B::Column>,
    row_index: usize,
    error: B::Error,
) -> B::Error {
    if backend.error_kind(&error) == InsertErrorKind::DataTooLong {
        return backend.load_data_truncated_error(
            &column
                .map(|column| backend.column_name(column))
                .unwrap_or_default(),
            row_index,
        );
    }
    error
}

/// 按是否 LOAD 语句选择 INSERT 或 LOAD 错误补全。
pub fn completeError<B: InsertBackend>(
    backend: &B,
    table: &B::Table,
    offset: usize,
    row_index: usize,
    error: B::Error,
) -> B::Error {
    backend.functional_index_error(table, offset, row_index, error)
}

/// 在表列中查找自增列下标。
pub fn findAutoIncrementColumn<B: InsertBackend>(
    backend: &B,
    table: &B::Table,
) -> Option<(B::Column, usize)> {
    backend
        .table_columns(table)
        .into_iter()
        .enumerate()
        .find_map(|(index, column)| {
            backend
                .column_is_auto_increment(&column)
                .then_some((column, index))
        })
}

/// 写入自增 ID 到 Datum 并按列类型 cast。
pub fn setDatumAutoIDAndCast<B: InsertBackend>(
    backend: &B,
    datum: &mut B::Datum,
    id: i64,
    column: &B::Column,
) -> Result<(), B::Error> {
    backend.datum_set_auto_id(datum, id, column);
    *datum = backend.cast_value(datum, column)?;
    if backend.datum_get_int64(datum) < id {
        if backend.truncate_as_warning_with_on_duplicate() {
            return Ok(());
        }
        return Err(backend.auto_increment_read_failed());
    }
    Ok(())
}

/// 从 Datum 提取用作 last_insert_id 的整型值。
pub fn getAutoRecordID<B: InsertBackend>(
    backend: &B,
    datum: &B::Datum,
    field_type: &B::FieldType,
    is_insert: bool,
) -> Result<i64, B::Error> {
    match backend.field_type_kind(field_type) {
        FieldKind::Float | FieldKind::Double => {
            let value = backend.datum_get_float64(datum);
            Ok(if is_insert {
                value.round() as i64
            } else {
                value as i64
            })
        }
        FieldKind::Integer => Ok(backend.datum_get_int64(datum)),
        _ => Err(backend.error("unexpected field type".to_owned())),
    }
}

/// DDL/辅助会话对象的类型擦除句柄。
type SessionObject = Box<dyn Any + Send + Sync>;
/// 创建辅助会话的回调类型。
pub type CreateSessionFn = dyn Fn(&dyn Any) -> Result<SessionObject, String> + Send + Sync;
/// 关闭辅助会话的回调类型。
pub type CloseSessionFn = dyn Fn(SessionObject) + Send + Sync;
/// 全局可注入的创建会话钩子。
pub static CreateSession: OnceLock<RwLock<Option<Arc<CreateSessionFn>>>> = OnceLock::new();
/// 全局可注入的关闭会话钩子。
pub static CloseSession: OnceLock<RwLock<Option<Arc<CloseSessionFn>>>> = OnceLock::new();

/// INSERT 运行时统计：基本耗时、快照与 ID 分配器统计。
pub struct InsertRuntimeStat<B: InsertBackend> {
    pub backend: Arc<B>,
    pub BasicRuntimeStats: Option<B::BasicRuntimeStats>,
    pub SnapshotRuntimeStats: Option<B::SnapshotRuntimeStats>,
    pub AllocatorRuntimeStats: Option<B::AllocatorRuntimeStats>,
    pub CheckInsertTime: Duration,
    pub Prefetch: Duration,
    pub FKCheckTime: Duration,
}

impl<B: InsertBackend> InsertRuntimeStat<B> {
    /// 格式化为可读统计字符串。
    pub fn String(&self) -> String {
        let allocator = self
            .AllocatorRuntimeStats
            .as_ref()
            .map(|stats| self.backend.allocator_stats_string(stats))
            .unwrap_or_default();
        let snapshot = self
            .SnapshotRuntimeStats
            .as_ref()
            .map(|stats| self.backend.snapshot_stats_string(stats))
            .unwrap_or_default();
        if self.CheckInsertTime == Duration::ZERO {
            let mut output = allocator;
            if self.Prefetch > Duration::ZERO && self.SnapshotRuntimeStats.is_some() {
                if !output.is_empty() {
                    output.push_str(", ");
                }
                let _ = write!(
                    output,
                    "prefetch: {}, rpc: {{{}}}",
                    self.backend.format_duration(self.Prefetch),
                    snapshot
                );
            }
            return output;
        }
        let total = self
            .BasicRuntimeStats
            .as_ref()
            .map(|stats| self.backend.basic_runtime_time(stats))
            .unwrap_or_default();
        let prepare = total.saturating_sub(self.CheckInsertTime);
        let mut output = if allocator.is_empty() {
            format!("prepare: {}, ", self.backend.format_duration(prepare))
        } else {
            format!(
                "prepare: {{total: {}, {}}}, ",
                self.backend.format_duration(prepare),
                allocator
            )
        };
        if self.Prefetch > Duration::ZERO {
            let _ = write!(
                output,
                "check_insert: {{total_time: {}, mem_insert_time: {}, prefetch: {}",
                self.backend.format_duration(self.CheckInsertTime),
                self.backend
                    .format_duration(self.CheckInsertTime.saturating_sub(self.Prefetch)),
                self.backend.format_duration(self.Prefetch)
            );
            if self.FKCheckTime > Duration::ZERO {
                let _ = write!(
                    output,
                    ", fk_check: {}",
                    self.backend.format_duration(self.FKCheckTime)
                );
            }
            if !snapshot.is_empty() {
                let _ = write!(output, ", rpc:{{{snapshot}}}");
            }
            output.push('}');
        } else {
            let _ = write!(
                output,
                "insert:{}",
                self.backend.format_duration(self.CheckInsertTime)
            );
            if !snapshot.is_empty() {
                let _ = write!(output, ", rpc:{{{snapshot}}}");
            }
        }
        output
    }

    /// 深拷贝统计快照（合并用）。
    pub fn Clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            BasicRuntimeStats: self.BasicRuntimeStats.clone(),
            SnapshotRuntimeStats: self
                .SnapshotRuntimeStats
                .as_ref()
                .map(|stats| self.backend.clone_snapshot_stats(stats)),
            AllocatorRuntimeStats: self
                .AllocatorRuntimeStats
                .as_ref()
                .map(|stats| self.backend.clone_allocator_stats(stats)),
            CheckInsertTime: self.CheckInsertTime,
            Prefetch: self.Prefetch,
            FKCheckTime: self.FKCheckTime,
        }
    }

    /// 合并另一份运行时统计。
    pub fn Merge(&mut self, other: &Self) {
        if let Some(source) = other.SnapshotRuntimeStats.as_ref() {
            match self.SnapshotRuntimeStats.as_mut() {
                Some(destination) => self.backend.merge_snapshot_stats(destination, source),
                None => self.SnapshotRuntimeStats = Some(self.backend.clone_snapshot_stats(source)),
            }
        }
        if self.BasicRuntimeStats.is_none() {
            self.BasicRuntimeStats = other.BasicRuntimeStats.clone();
        }
        if let Some(source) = other.AllocatorRuntimeStats.as_ref() {
            match self.AllocatorRuntimeStats.as_mut() {
                Some(destination) => self.backend.merge_allocator_stats(destination, source),
                None => {
                    self.AllocatorRuntimeStats = Some(self.backend.clone_allocator_stats(source))
                }
            }
        }
        self.Prefetch += other.Prefetch;
        self.FKCheckTime += other.FKCheckTime;
        self.CheckInsertTime += other.CheckInsertTime;
    }

    /// 返回统计类型标识。
    pub fn Tp(&self) -> i32 {
        self.backend.insert_runtime_stats_type()
    }
}

/// Provider evaluation is separated from session-bound argument preparation.
/// Results retain row/column order; provider failures do not cancel sibling work,
/// while request cancellation is checked even for NULL inputs.
pub fn evaluate_embedding_inputs(
    evaluate: &(dyn Fn(&astersql_expression::EmbedTextArgs) -> Result<Vec<f32>, String> + Sync),
    inputs: &[Result<Option<astersql_expression::EmbedTextArgs>, String>],
    cancellation: &(dyn Fn() -> Option<String> + Sync),
) -> Result<Vec<Result<Option<Vec<f32>>, String>>, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(vec![Ok(None); inputs.len()]);
    let canceled = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..inputs.len().min(800) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= inputs.len() {
                        break;
                    }
                    if let Some(error) = cancellation() {
                        let mut cause = canceled.lock().unwrap();
                        if cause.is_none() {
                            *cause = Some(error);
                        }
                        break;
                    }
                    let result = match &inputs[index] {
                        Err(error) => Err(error.clone()),
                        Ok(None) => Ok(None),
                        Ok(Some(args)) => evaluate(args).map(Some),
                    };
                    results.lock().unwrap()[index] = result;
                }
            });
        }
    });
    if let Some(error) = canceled.into_inner().unwrap() {
        return Err(error);
    }
    Ok(results.into_inner().unwrap())
}
