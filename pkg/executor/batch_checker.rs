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

// 批量唯一键 / 主键冲突检查（batch checker）。
//
// 对应 Go 的 batch checker：在 INSERT/REPLACE/UPDATE 写路径上，
// 预先为每一行收集需要探测的 record key 与 unique index key，并构造
// 冲突时的重复键错误信息。真实表元数据、键编码与事务读通过
// `BatchCheckerRuntime` 注入，执行器本身只拥有去重检查算法。

#![allow(non_snake_case)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Datum（列值）的粗分类，用于把二进制/位串等格式化为可读错误文本。
pub enum DatumKind {
    Bytes,
    MysqlBit,
    BinaryLiteral,
    Other,
}

/// 待检查的键及其对应的“重复键”错误对象。
pub struct KeyValueWithDupInfo<K, E> {
    pub new_key: K,
    pub dup_err: E,
}

/// 一行待查数据：可选主键/句柄键、唯一索引键列表，以及所属表或分区。
pub struct ToBeCheckedRow<Row, K, E, T> {
    pub row: Row,
    pub handle_key: Option<KeyValueWithDupInfo<K, E>>,
    pub unique_keys: Vec<KeyValueWithDupInfo<K, E>>,
    /// 该行所属的具体表或分区。
    /// The concrete table or partition to which this row belongs.
    pub table: Option<T>,
    pub ignored: bool,
}

/// 由索引值生成的索引键；`distinct` 表示该键在批内是否需去重检查。
pub struct GeneratedIndexKey<K> {
    pub key: K,
    pub distinct: bool,
}

/// 表元数据、键编码与事务读取的生产边界。
///
/// 执行器拥有重复键检查算法；实现方需把这些操作接到真实的
/// table/kv/session 设施上，不存在默认成功实现。
/// Production boundary for table metadata, key encoding and transaction reads.
///
/// The executor owns the duplicate-check algorithm. Implementations must wire
/// these operations to the real table/kv/session facilities; there is no
/// success-by-default implementation.
pub trait BatchCheckerRuntime {
    type Context;
    type Table: Clone;
    type Row;
    type Datum: Clone;
    type Column: Clone;
    type Index: Clone;
    type Handle: Clone;
    type Key;
    type Transaction;
    type Expression;
    type Error;

    /// 返回表上可写索引列表。
    fn writable_indices(&self, table: &Self::Table) -> Vec<Self::Index>;
    /// 索引当前状态是否允许写入。
    fn index_is_writable(&self, index: &Self::Index) -> bool;
    /// 是否为唯一索引（含主键索引）。
    fn index_is_unique(&self, index: &Self::Index) -> bool;
    /// 是否为主键索引。
    fn index_is_primary(&self, index: &Self::Index) -> bool;
    /// 索引是否已对查询可见（public 状态）。
    fn index_is_public(&self, index: &Self::Index) -> bool;
    /// 回填（backfill）是否不适用；影响临时索引键编码选择。
    fn index_backfill_is_inapplicable(&self, index: &Self::Index) -> bool;
    /// 索引名，用于错误信息。
    fn index_name(&self, index: &Self::Index) -> String;
    /// 索引所属表名。
    fn index_table_name(&self, index: &Self::Index) -> String;
    /// 返回主键索引（常见于 common handle 表）。
    fn primary_index(&self, table: &Self::Table) -> Option<Self::Index>;
    /// 主键索引列在表列中的偏移列表。
    fn primary_index_columns(&self, index: &Self::Index) -> Vec<usize>;

    /// 表全部列定义。
    fn table_columns(&self, table: &Self::Table) -> Vec<Self::Column>;
    /// 当前可写列（含写状态变更中的列）。
    fn writable_columns(&self, table: &Self::Table) -> Vec<Self::Column>;
    /// 表名。
    fn table_name(&self, table: &Self::Table) -> String;
    /// 主键是否直接作为行句柄（integer handle）。
    fn table_pk_is_handle(&self, table: &Self::Table) -> bool;
    /// 是否为 common handle（聚簇索引主键编码为字节句柄）。
    fn table_is_common_handle(&self, table: &Self::Table) -> bool;
    /// 列是否为整型主键句柄列。
    fn column_is_pk_handle(&self, table: &Self::Table, column: &Self::Column) -> bool;
    /// 列在行中的偏移。
    fn column_offset(&self, column: &Self::Column) -> usize;
    /// 列 ID。
    fn column_id(&self, column: &Self::Column) -> i64;
    /// 变更列依赖的源列偏移。
    fn column_dependency_offset(&self, column: &Self::Column) -> usize;
    /// 列是否处于 DDL 变更状态。
    fn column_has_change_state(&self, column: &Self::Column) -> bool;
    /// 列是否 public。
    fn column_is_public(&self, column: &Self::Column) -> bool;
    /// 是否为生成列（generated column）。
    fn column_is_generated(&self, column: &Self::Column) -> bool;
    /// 生成列是否为 STORED（相对 VIRTUAL）。
    fn column_is_generated_stored(&self, column: &Self::Column) -> bool;

    /// 按行值定位所属分区表；无分区时返回原表。
    fn partition_for_row(
        &mut self,
        context: &mut Self::Context,
        table: &Self::Table,
        row: &Self::Row,
    ) -> Result<Option<Self::Table>, Self::Error>;
    /// 仅当 ErrCtx 接受该分区错误时返回 Ok；否则向上传播。
    /// Return `Ok(())` only for a partition error accepted by ErrCtx.
    fn handle_partition_error(&mut self, error: Self::Error) -> Result<(), Self::Error>;

    /// 读取行中指定偏移的 datum。
    fn row_datum<'a>(&self, row: &'a Self::Row, offset: usize) -> &'a Self::Datum;
    /// 可变读取行中指定偏移的 datum。
    fn row_datum_mut<'a>(&self, row: &'a mut Self::Row, offset: usize) -> &'a mut Self::Datum;
    /// 行当前列数。
    fn row_len(&self, row: &Self::Row) -> usize;
    /// 向行末追加一列（用于临时补齐写状态列）。
    fn row_push(&self, row: &mut Self::Row, datum: Self::Datum);
    /// 截断行到指定长度，撤销临时追加列。
    fn row_truncate(&self, row: &mut Self::Row, length: usize);
    /// 将 datum 视为 i64 句柄值。
    fn datum_int64(&self, datum: &Self::Datum) -> i64;
    /// datum 是否为 NULL。
    fn datum_is_null(&self, datum: &Self::Datum) -> bool;
    /// 返回 datum 粗分类。
    fn datum_kind(&self, datum: &Self::Datum) -> DatumKind;
    /// 将 datum 转为错误信息用字符串。
    fn datum_to_string(&self, datum: &Self::Datum) -> Result<String, Self::Error>;
    /// 将不可打印字节显示为十六进制。
    fn printable_non_ascii_as_hex(&self, value: String) -> String;

    /// 由整型构造行句柄（handle）。
    fn int_handle(&self, value: i64) -> Self::Handle;
    /// 从句柄反解出组成主键的 datum 列表。
    fn handle_data(&self, handle: &Self::Handle) -> Result<Vec<Self::Datum>, Self::Error>;
    /// 编码行记录键（record key）。
    fn encode_record_key(&self, table: &Self::Table, handle: &Self::Handle) -> Self::Key;
    /// 构造带键名与值文本的重复键错误。
    fn duplicate_error(&self, values: Vec<String>, key_name: String) -> Self::Error;
    /// 无法格式化键值时的通用重复键错误。
    fn generic_duplicate_error(&self) -> Self::Error;
    /// 记录键值字符串化失败日志。
    fn log_key_string_failure(&self, error: &Self::Error, handle: &Self::Handle);

    /// 对变更中的列做类型转换。
    fn cast_changing_column(
        &mut self,
        context: &mut Self::Context,
        datum: &Self::Datum,
        column: &Self::Column,
    ) -> Result<Self::Datum, Self::Error>;
    /// 取列的原始默认值（write-only 列补齐用）。
    fn column_origin_default(
        &mut self,
        context: &mut Self::Context,
        column: &Self::Column,
    ) -> Result<Self::Datum, Self::Error>;
    /// 部分索引（partial index）条件是否满足。
    fn index_meets_partial_condition(
        &mut self,
        context: &mut Self::Context,
        index: &Self::Index,
        row: &Self::Row,
    ) -> Result<bool, Self::Error>;
    /// 从行中抽取索引列值。
    fn index_values(
        &mut self,
        index: &Self::Index,
        row: &Self::Row,
    ) -> Result<Vec<Self::Datum>, Self::Error>;
    /// 由索引列值生成索引键（可能一对多，如前缀索引）。
    fn generate_index_keys(
        &mut self,
        context: &mut Self::Context,
        index: &Self::Index,
        values: &[Self::Datum],
    ) -> Result<Vec<GeneratedIndexKey<Self::Key>>, Self::Error>;
    /// 非 public 索引回填阶段使用的临时索引键编码。
    fn temporary_index_key(&self, index: &Self::Index, key: Self::Key) -> Self::Key;

    /// 按索引前缀长度截断 datum。
    fn truncate_index_datum(
        &mut self,
        datum: &mut Self::Datum,
        index: &Self::Index,
        index_column: usize,
        table_column: &Self::Column,
    );
    /// 将主键列值编码为 common handle。
    fn encode_common_handle(
        &mut self,
        context: &mut Self::Context,
        values: Vec<Self::Datum>,
    ) -> Result<Self::Handle, Self::Error>;

    /// 在事务快照中按键读取原始值。
    fn transaction_get(
        &mut self,
        context: &mut Self::Context,
        transaction: &mut Self::Transaction,
        key: &Self::Key,
    ) -> Result<Vec<u8>, Self::Error>;
    /// 解码原始行字节为行对象，并返回实际存在的列 ID。
    fn decode_raw_row(
        &mut self,
        context: &mut Self::Context,
        table: &Self::Table,
        handle: &Self::Handle,
        columns: &[Self::Column],
        value: Vec<u8>,
    ) -> Result<(Self::Row, Vec<i64>), Self::Error>;
    /// 求值虚拟生成列表达式。
    fn evaluate_generated_column(
        &mut self,
        context: &mut Self::Context,
        expression: &Self::Expression,
        row: &Self::Row,
    ) -> Result<Self::Datum, Self::Error>;
    /// 将生成列结果转换到目标列类型。
    fn cast_generated_column(
        &mut self,
        context: &mut Self::Context,
        value: Self::Datum,
        column: &Self::Column,
    ) -> Result<Self::Datum, Self::Error>;
}

/// 批量为多行收集需要查重的主键/唯一索引键。
pub fn getKeysNeedCheck<R: BatchCheckerRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    table: R::Table,
    rows: Vec<R::Row>,
) -> Result<Vec<ToBeCheckedRow<R::Row, R::Key, R::Error, R::Table>>, R::Error> {
    // 预估唯一索引数量，便于为 unique_keys 预分配容量
    let unique_count = runtime
        .writable_indices(&table)
        .iter()
        .filter(|index| runtime.index_is_writable(index) && runtime.index_is_unique(index))
        .count();

    // 确定句柄列：整型 PK handle，或 common handle 的主键索引列
    let mut handle_columns = Vec::new();
    let mut primary_index = None;
    if runtime.table_pk_is_handle(&table) {
        if let Some(column) = runtime
            .table_columns(&table)
            .into_iter()
            .find(|column| runtime.column_is_pk_handle(&table, column))
        {
            handle_columns.push(column);
        }
    } else if runtime.table_is_common_handle(&table) {
        primary_index = runtime.primary_index(&table);
        if let Some(index) = &primary_index {
            let columns = runtime.table_columns(&table);
            for offset in runtime.primary_index_columns(index) {
                handle_columns.push(columns[offset].clone());
            }
        }
    }

    let mut checked = Vec::with_capacity(rows.len());
    for row in rows {
        getKeysNeedCheckOneRow(
            runtime,
            context,
            table.clone(),
            row,
            unique_count,
            &handle_columns,
            primary_index.as_ref(),
            &mut checked,
        )?;
    }
    Ok(checked)
}

#[allow(clippy::too_many_arguments)]
/// 为单行解析分区、构造句柄键与唯一索引键，并处理写状态列补齐。
pub fn getKeysNeedCheckOneRow<R: BatchCheckerRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    mut table: R::Table,
    mut row: R::Row,
    unique_count: usize,
    handle_columns: &[R::Column],
    primary_index: Option<&R::Index>,
    result: &mut Vec<ToBeCheckedRow<R::Row, R::Key, R::Error, R::Table>>,
) -> Result<(), R::Error> {
    // 分区定位失败且 ErrCtx 忽略时，标记 ignored 并跳过后续检查
    match runtime.partition_for_row(context, &table, &row) {
        Ok(Some(partition)) => table = partition,
        Ok(None) => {}
        Err(error) => {
            runtime.handle_partition_error(error)?;
            result.push(ToBeCheckedRow {
                row,
                handle_key: None,
                unique_keys: Vec::new(),
                table: None,
                ignored: true,
            });
            return Ok(());
        }
    }

    // 构造行句柄：common handle 由主键列编码，否则取整型 PK
    let handle = if runtime.table_is_common_handle(&table) {
        Some(buildHandleFromDatumRow(
            runtime,
            context,
            &row,
            handle_columns,
            primary_index,
        )?)
    } else if let Some(column) = handle_columns.first() {
        Some(runtime.int_handle(
            runtime.datum_int64(runtime.row_datum(&row, runtime.column_offset(column))),
        ))
    } else {
        None
    };

    // 编码 record key，并尽量把主键值格式化为重复键错误文本
    let handle_key = if let Some(handle) = &handle {
        let values = if runtime.table_is_common_handle(&table) {
            handle_columns
                .iter()
                .map(|column| {
                    runtime
                        .row_datum(&row, runtime.column_offset(column))
                        .clone()
                })
                .collect()
        } else {
            vec![
                runtime
                    .row_datum(&row, runtime.column_offset(&handle_columns[0]))
                    .clone(),
            ]
        };
        let strings = dataToStrings(runtime, &values).or_else(|error| {
            let fallback = runtime
                .handle_data(handle)
                .and_then(|data| dataToStrings(runtime, &data));
            if fallback.is_err() {
                runtime.log_key_string_failure(&error, handle);
            }
            fallback
        });
        let duplicate = match strings {
            Ok(strings) => {
                runtime.duplicate_error(strings, format!("{}.PRIMARY", runtime.table_name(&table)))
            }
            Err(error) => {
                runtime.log_key_string_failure(&error, handle);
                runtime.generic_duplicate_error()
            }
        };
        Some(KeyValueWithDupInfo {
            new_key: runtime.encode_record_key(&table, handle),
            dup_err: duplicate,
        })
    } else {
        None
    };

    // 临时追加变更中/默认列值，供索引取值；结束后截断回原始长度
    let original_length = runtime.row_len(&row);
    for column in runtime.writable_columns(&table) {
        if runtime.column_has_change_state(&column) && !runtime.column_is_public(&column) {
            let value = runtime.cast_changing_column(
                context,
                runtime.row_datum(&row, runtime.column_dependency_offset(&column)),
                &column,
            )?;
            runtime.row_push(&mut row, value);
        } else if !runtime.column_is_public(&column)
            && runtime.column_offset(&column) >= runtime.row_len(&row)
        {
            let value = runtime.column_origin_default(context, &column)?;
            runtime.row_push(&mut row, value);
        }
    }

    // 遍历可写唯一索引，跳过主键（common handle）与不满足部分索引条件者
    let mut unique_keys = Vec::with_capacity(unique_count);
    for index in runtime.writable_indices(&table) {
        if !runtime.index_is_writable(&index)
            || !runtime.index_is_unique(&index)
            || (runtime.table_is_common_handle(&table) && runtime.index_is_primary(&index))
            || !runtime.index_meets_partial_condition(context, &index, &row)?
        {
            continue;
        }
        let values = runtime.index_values(&index, &row)?;
        for generated in runtime.generate_index_keys(context, &index, &values)? {
            if !generated.distinct {
                continue;
            }
            let key = if !runtime.index_is_public(&index)
                && !runtime.index_backfill_is_inapplicable(&index)
            {
                runtime.temporary_index_key(&index, generated.key)
            } else {
                generated.key
            };
            let duplicate = runtime.duplicate_error(
                dataToStrings(runtime, &values)?,
                format!(
                    "{}.{}",
                    runtime.index_table_name(&index),
                    runtime.index_name(&index)
                ),
            );
            unique_keys.push(KeyValueWithDupInfo {
                new_key: key,
                dup_err: duplicate,
            });
        }
    }
    runtime.row_truncate(&mut row, original_length);
    result.push(ToBeCheckedRow {
        row,
        handle_key,
        unique_keys,
        table: Some(table),
        ignored: false,
    });
    Ok(())
}

/// 从行的主键列值构建 common handle。
pub fn buildHandleFromDatumRow<R: BatchCheckerRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    row: &R::Row,
    handle_columns: &[R::Column],
    primary_index: Option<&R::Index>,
) -> Result<R::Handle, R::Error> {
    let mut values = Vec::with_capacity(handle_columns.len());
    for (position, column) in handle_columns.iter().enumerate() {
        let mut datum = runtime
            .row_datum(row, runtime.column_offset(column))
            .clone();
        if let Some(index) = primary_index {
            if !runtime.primary_index_columns(index).is_empty() {
                runtime.truncate_index_datum(&mut datum, index, position, column);
            }
        }
        values.push(datum);
    }
    runtime.encode_common_handle(context, values)
}

/// 将一组 datum 转为可读字符串列表，供重复键错误使用。
pub fn dataToStrings<R: BatchCheckerRuntime>(
    runtime: &R,
    data: &[R::Datum],
) -> Result<Vec<String>, R::Error> {
    let mut strings = Vec::with_capacity(data.len());
    for datum in data {
        let mut value = runtime.datum_to_string(datum)?;
        match runtime.datum_kind(datum) {
            // 二进制类值：去掉尾部 NUL 后按十六进制展示不可打印字符
            DatumKind::Bytes | DatumKind::MysqlBit | DatumKind::BinaryLiteral => {
                if runtime.datum_kind(datum) == DatumKind::Bytes {
                    value = value.trim_end_matches('\0').to_owned();
                    if value.is_empty() {
                        value.push('\0');
                    }
                }
                value = runtime.printable_non_ascii_as_hex(value);
            }
            DatumKind::Other => {}
        }
        strings.push(value);
    }
    Ok(strings)
}

/// 加载已有记录：补齐缺失的 write-only 列，并按原顺序求值 public 虚拟生成列。
/// Load an existing record, restore missing write-only columns and evaluate
/// public virtual generated columns in their original order.
pub fn getOldRow<R: BatchCheckerRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    transaction: &mut R::Transaction,
    table: &R::Table,
    handle: &R::Handle,
    generated_expressions: &[R::Expression],
) -> Result<R::Row, R::Error> {
    // 事务内点查记录，再按可写列解码
    let key = runtime.encode_record_key(table, handle);
    let value = runtime.transaction_get(context, transaction, &key)?;
    let columns = runtime.writable_columns(table);
    let (mut row, present_column_ids) =
        runtime.decode_raw_row(context, table, handle, &columns, value)?;
    // 按列顺序：补默认值；对 public 虚拟生成列求值并写回
    let mut generated_index = 0;
    for column in columns {
        let offset = runtime.column_offset(&column);
        if !runtime.column_is_public(&column)
            && runtime.datum_is_null(runtime.row_datum(&row, offset))
            && !present_column_ids.contains(&runtime.column_id(&column))
        {
            let value = runtime.column_origin_default(context, &column)?;
            *runtime.row_datum_mut(&mut row, offset) = value;
        }
        if runtime.column_is_generated(&column) && runtime.column_is_public(&column) {
            if !runtime.column_is_generated_stored(&column) {
                let value = runtime.evaluate_generated_column(
                    context,
                    &generated_expressions[generated_index],
                    &row,
                )?;
                let value = runtime.cast_generated_column(context, value, &column)?;
                *runtime.row_datum_mut(&mut row, offset) = value;
            }
            generated_index += 1;
        }
    }
    Ok(row)
}
