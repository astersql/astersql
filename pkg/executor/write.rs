// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// DML 写路径公共逻辑：UPDATE 行比较、ON UPDATE、外键与分区交换校验。
//
// `updateRecord` 对应 Go 侧同名流程：对比新旧行、回填时间戳与赋值表达式，
// 在主键/聚簇索引 handle 变更时走 replace，否则 update；悲观事务下可锁未变键。
// Handle 是行在存储中的主键定位键；悲观事务在执行阶段加锁，提交前避免写冲突。

#![allow(non_snake_case, non_upper_case_globals)]

use std::fmt;

/// 未变更行加锁时包含行主键（row key）的位标志。
pub const lockRowKey: u8 = 1 << 0;
/// 未变更行加锁时包含唯一索引键的位标志。
pub const lockUniqueKeys: u8 = 1 << 1;

/// 写执行器对会话/表元数据/存储的依赖边界（列属性、赋值求值、外键与分区检查）。
pub trait WriteRuntime {
    /// 执行上下文（会话变量、事务状态等）。
    type Context;
    /// 列值（Datum）类型。
    type Datum: Clone;
    /// 列元信息类型。
    type Column: Clone;
    /// UPDATE 赋值项类型。
    type Assignment;
    /// 行 handle（主键定位键）类型。
    type Handle: Clone;
    /// 唯一键冲突处理模式。
    type DuplicateKeyMode;
    /// 外键引用检查项。
    type ForeignKeyCheck;
    /// 外键级联更新项。
    type ForeignKeyCascade;
    /// 写路径错误类型。
    type Error;

    /// 返回表全部列元信息。
    fn columns(&self) -> Vec<Self::Column>;
    /// 列是否为生成列（含存储型）。
    fn column_is_generated(&self, column: &Self::Column) -> bool;
    /// 列是否为虚拟生成列。
    fn column_is_virtual_generated(&self, column: &Self::Column) -> bool;
    /// 列是否带 ON UPDATE CURRENT_TIMESTAMP。
    fn column_is_on_update_now(&self, column: &Self::Column) -> bool;
    /// 列是否自增。
    fn column_is_auto_increment(&self, column: &Self::Column) -> bool;
    /// 列是否为整型主键 handle（pk-is-handle）。
    fn column_is_pk_handle(&self, column: &Self::Column) -> bool;
    /// 列是否参与聚簇索引 common handle。
    fn column_is_common_handle(&self, column: &Self::Column) -> bool;
    /// 列是否 NOT NULL。
    fn column_is_not_null(&self, column: &Self::Column) -> bool;
    /// 列是否禁止以 NULL 插入（严格模式相关）。
    fn column_prevents_null_insert(&self, column: &Self::Column) -> bool;
    /// 列名，用于错误信息。
    fn column_name(&self, column: &Self::Column) -> String;
    /// Datum 是否为 SQL NULL。
    fn datum_is_null(&self, datum: &Self::Datum) -> bool;
    /// 求值失败时占位用的零值 Datum。
    fn zero_datum(&self) -> Self::Datum;
    /// 二进制语义比较两列值是否相等。
    fn compare_binary(
        &mut self,
        left: &Self::Datum,
        right: &Self::Datum,
    ) -> Result<std::cmp::Ordering, Self::Error>;
    /// 自增列写入更大值时回填自增计数器。
    fn rebase_auto_increment(
        &mut self,
        context: &mut Self::Context,
        column: &Self::Column,
        datum: &Self::Datum,
    ) -> Result<(), Self::Error>;
    /// 表是否启用 AUTO_RANDOM。
    fn table_contains_auto_random_bits(&self) -> bool;
    /// AUTO_RANDOM 增量部分掩码。
    fn auto_random_incremental_mask(&self, column: &Self::Column) -> i64;
    /// 从 Datum 解析 AUTO_RANDOM 记录 id。
    fn auto_record_id(
        &self,
        column: &Self::Column,
        datum: &Self::Datum,
    ) -> Result<i64, Self::Error>;
    /// 回填 AUTO_RANDOM 分配水位。
    fn rebase_auto_random(
        &mut self,
        context: &mut Self::Context,
        value: i64,
    ) -> Result<(), Self::Error>;

    /// 客户端是否开启 CLIENT_FOUND_ROWS（未改行也计入 affected）。
    fn client_found_rows(&self) -> bool;
    /// 是否对未变更唯一键也加锁。
    fn lock_unchanged_keys(&self) -> bool;
    /// 当前是否处于悲观事务。
    fn in_pessimistic_transaction(&self) -> bool;
    /// 将未变更行的键加入悲观锁集合。
    fn add_unchanged_keys_for_lock(
        &mut self,
        context: &mut Self::Context,
        handle: &Self::Handle,
        row: &[Self::Datum],
        key_set: u8,
    ) -> Result<usize, Self::Error>;
    /// 累加 touched 行数（扫描/触及）。
    fn add_touched_rows(&mut self, rows: u64);
    /// 累加 affected 行数。
    fn add_affected_rows(&mut self, rows: u64);
    /// 累加真正更新的行数。
    fn add_updated_rows(&mut self, rows: u64);
    /// 累加复制写入的行数（MySQL 兼容统计）。
    fn add_copied_rows(&mut self, rows: u64);

    /// 取当前时间戳填充 ON UPDATE 列。
    fn current_timestamp(
        &mut self,
        context: &mut Self::Context,
        column: &Self::Column,
    ) -> Result<Self::Datum, Self::Error>;
    /// 构造 ON UPDATE 列不应作为整型主键 handle 的内部一致性错误。
    fn on_update_now_pk_handle_error(&self) -> Self::Error;
    /// 是否存在赋值求值缓冲区。
    fn evaluation_buffer_exists(&self) -> bool;
    /// 将 Datum 写入求值缓冲区指定下标。
    fn set_evaluation_buffer_datum(&mut self, index: usize, datum: Self::Datum);
    /// 赋值项上延迟绑定的错误（若有）。
    fn assignment_lazy_error(&mut self, assignment: &Self::Assignment) -> Option<Self::Error>;
    /// 赋值目标列在缓冲区中的下标。
    fn assignment_column_index(&self, assignment: &Self::Assignment) -> usize;
    /// 求值赋值表达式得到原始 Datum。
    fn evaluate_assignment(
        &mut self,
        context: &mut Self::Context,
        assignment: &Self::Assignment,
    ) -> Result<Self::Datum, Self::Error>;
    /// 将赋值结果转换为目标列类型。
    fn cast_assignment(
        &mut self,
        context: &mut Self::Context,
        assignment: &Self::Assignment,
        value: &Self::Datum,
    ) -> Result<Self::Datum, Self::Error>;

    /// 构造坏 NULL（违反 NOT NULL）错误。
    fn bad_null_error(&self, column: &Self::Column) -> Self::Error;
    /// 处理坏 NULL：报错或按模式改写。
    fn handle_bad_null(
        &mut self,
        column: &Self::Column,
        datum: &mut Self::Datum,
    ) -> Result<(), Self::Error>;
    /// IGNORE 语义下检查外键，返回是否应跳过本行。
    fn check_fk_ignore_error(
        &mut self,
        context: &mut Self::Context,
        checks: &mut [Self::ForeignKeyCheck],
        row: &[Self::Datum],
    ) -> Result<bool, Self::Error>;
    /// 是否需要校验交换分区行落点。
    fn exchange_partition_check_required(&self) -> bool;
    /// 校验行是否满足交换分区目标分区约束。
    fn check_exchange_partition_row(
        &mut self,
        context: &mut Self::Context,
        row: &[Self::Datum],
    ) -> Result<(), Self::Error>;

    /// handle 变更时用 staging 替换整行记录。
    fn replace_record_with_staging(
        &mut self,
        context: &mut Self::Context,
        handle: &Self::Handle,
        old_data: &[Self::Datum],
        new_data: &[Self::Datum],
        duplicate_key_mode: &Self::DuplicateKeyMode,
    ) -> Result<(), Self::Error>;
    /// 当前是否在显式事务中。
    fn in_transaction(&self) -> bool;
    /// 当前是否处于外键触发器执行路径。
    fn in_foreign_key_trigger(&self) -> bool;
    /// 是否存在外键级联动作。
    fn has_foreign_key_cascades(&self) -> bool;
    #[allow(clippy::too_many_arguments)]
    /// handle 未变时更新记录（可跳过未触及索引）。
    fn update_record(
        &mut self,
        context: &mut Self::Context,
        handle: &Self::Handle,
        old_data: &[Self::Datum],
        new_data: &[Self::Datum],
        modified: &[bool],
        duplicate_key_mode: &Self::DuplicateKeyMode,
        skip_untouched_indices: bool,
    ) -> Result<(), Self::Error>;
    /// 将分区相关存储错误映射为对外错误。
    fn handle_partition_error(&mut self, error: Self::Error) -> Self::Error;
    /// 更新后执行外键引用检查。
    fn foreign_key_update_check(
        &mut self,
        check: &mut Self::ForeignKeyCheck,
        old_data: &[Self::Datum],
        new_data: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    /// 更新后执行外键级联。
    fn foreign_key_update_cascade(
        &mut self,
        cascade: &mut Self::ForeignKeyCascade,
        old_data: &[Self::Datum],
        new_data: &[Self::Datum],
    ) -> Result<(), Self::Error>;
}

#[allow(clippy::too_many_arguments)]
/// 更新单行：比较新旧值、应用 ON UPDATE/赋值、校验外键与分区，再写存储。
///
/// 返回 `(changed, skipped)`：`changed` 表示是否真正写入；`skipped` 表示 IGNORE 跳过。
pub fn updateRecord<R, H>(
    runtime: &mut R,
    context: &mut R::Context,
    handle: R::Handle,
    old_data: Vec<R::Datum>,
    mut new_data: Vec<R::Datum>,
    offset: usize,
    assignments: &[R::Assignment],
    mut error_handler: H,
    mut modified: Vec<bool>,
    on_duplicate: bool,
    foreign_key_checks: &mut [R::ForeignKeyCheck],
    foreign_key_cascades: &mut [R::ForeignKeyCascade],
    duplicate_key_mode: &R::DuplicateKeyMode,
    ignore_error: bool,
) -> Result<(bool, bool), R::Error>
where
    R: WriteRuntime,
    H: FnMut(&R::Assignment, &mut R::Datum, Option<R::Error>) -> Result<(), R::Error>,
{
    let columns = runtime.columns();
    assert_eq!(old_data.len(), columns.len());
    assert_eq!(new_data.len(), columns.len());
    assert_eq!(modified.len(), columns.len());

    let mut changed = false;
    let mut handle_changed = false;
    let mut on_update_needs_modification = vec![false; columns.len()];

    // 第一遍：跳过生成列，标记 ON UPDATE，比较新旧值并回填自增/AUTO_RANDOM。
    for (index, column) in columns.iter().enumerate() {
        if runtime.column_is_generated(column) {
            continue;
        }
        if runtime.column_is_on_update_now(column) {
            on_update_needs_modification[index] = !modified[index];
        }
        let different = runtime.compare_binary(&new_data[index], &old_data[index])?
            != std::cmp::Ordering::Equal;
        modified[index] = different;
        if !different {
            continue;
        }
        changed = true;
        if runtime.column_is_auto_increment(column) {
            runtime.rebase_auto_increment(context, column, &new_data[index])?;
        }
        if runtime.column_is_pk_handle(column) {
            handle_changed = true;
            rebaseAutoRandomValue(runtime, context, &new_data[index], column)?;
        }
        handle_changed |= runtime.column_is_common_handle(column);
    }

    // 行未变化：按 CLIENT_FOUND_ROWS 统计，并在悲观事务中锁未变键后返回。
    if !changed {
        runtime.add_touched_rows(1);
        if runtime.client_found_rows() {
            runtime.add_affected_rows(1);
        }
        let mut key_set = lockRowKey;
        if runtime.lock_unchanged_keys() {
            key_set |= lockUniqueKeys;
        }
        addUnchangedKeysForLockByRow(runtime, context, &handle, &old_data, key_set)?;
        return Ok((false, false));
    }

    // 为仍需刷新的 ON UPDATE CURRENT_TIMESTAMP 列填入当前时间。
    for (index, column) in columns.iter().enumerate() {
        if runtime.column_is_on_update_now(column) && on_update_needs_modification[index] {
            new_data[index] = runtime.current_timestamp(context, column)?;
            modified[index] = true;
            if runtime.evaluation_buffer_exists() {
                runtime.set_evaluation_buffer_datum(index + offset, new_data[index].clone());
            }
            if runtime.column_is_pk_handle(column) {
                return Err(runtime.on_update_now_pk_handle_error());
            }
            handle_changed |= runtime.column_is_common_handle(column);
        }
    }

    // 求值 SET 赋值列表，cast 后写回 new_data，并再次检测 handle 是否变化。
    for assignment in assignments {
        if let Some(error) = runtime.assignment_lazy_error(assignment) {
            return Err(error);
        }
        let assignment_index = runtime.assignment_column_index(assignment);
        let column_index = assignment_index - offset;
        let (mut raw_value, mut assignment_error) =
            match runtime.evaluate_assignment(context, assignment) {
                Ok(value) => (value, None),
                Err(error) => (runtime.zero_datum(), Some(error)),
            };
        if assignment_error.is_none() {
            match runtime.cast_assignment(context, assignment, &raw_value) {
                Ok(value) => new_data[column_index] = value,
                Err(error) => assignment_error = Some(error),
            }
        }
        runtime.set_evaluation_buffer_datum(assignment_index, new_data[column_index].clone());
        error_handler(assignment, &mut raw_value, assignment_error)?;

        let column = &columns[column_index];
        if runtime.column_is_on_update_now(column) {
            on_update_needs_modification[column_index] = !modified[column_index];
        }
        let different = runtime.compare_binary(&new_data[column_index], &old_data[column_index])?
            != std::cmp::Ordering::Equal;
        modified[column_index] = different;
        if different {
            changed = true;
            if runtime.column_is_auto_increment(column) {
                runtime.rebase_auto_increment(context, column, &new_data[column_index])?;
            }
            if runtime.column_is_pk_handle(column) {
                handle_changed = true;
                rebaseAutoRandomValue(runtime, context, &new_data[column_index], column)?;
            }
            handle_changed |= runtime.column_is_common_handle(column);
        }
    }

    // 虚拟生成列旧值为 NULL 且列禁止 NULL 时，直接报坏 NULL。
    for (index, column) in columns.iter().enumerate() {
        if runtime.column_is_virtual_generated(column)
            && runtime.datum_is_null(&old_data[index])
            && (runtime.column_is_not_null(column) || runtime.column_prevents_null_insert(column))
        {
            return Err(runtime.bad_null_error(column));
        }
    }

    // IGNORE 路径下外键冲突则跳过本行。
    if ignore_error && runtime.check_fk_ignore_error(context, foreign_key_checks, &new_data)? {
        return Ok((false, true));
    }
    for (index, column) in columns.iter().enumerate() {
        runtime.handle_bad_null(column, &mut new_data[index])?;
    }
    if runtime.exchange_partition_check_required() {
        checkRowForExchangePartition(runtime, context, &new_data)?;
    }

    runtime.add_touched_rows(1);
    // handle 变化走 replace；否则 update，并可锁未变唯一键。
    if handle_changed {
        if let Err(error) = runtime.replace_record_with_staging(
            context,
            &handle,
            &old_data,
            &new_data,
            duplicate_key_mode,
        ) {
            return Err(runtime.handle_partition_error(error));
        }
    } else {
        // 非事务且无外键路径时可跳过未触及索引维护。
        let skip_untouched_indices = !(runtime.in_transaction()
            || runtime.in_foreign_key_trigger()
            || runtime.has_foreign_key_cascades());
        if let Err(error) = runtime.update_record(
            context,
            &handle,
            &old_data,
            &new_data,
            &modified,
            duplicate_key_mode,
            skip_untouched_indices,
        ) {
            return Err(runtime.handle_partition_error(error));
        }
        if runtime.lock_unchanged_keys() {
            addUnchangedKeysForLockByRow(runtime, context, &handle, &old_data, lockUniqueKeys)?;
        }
    }

    // 非 IGNORE 时做外键检查，再执行级联，并更新 affected/updated/copied 计数。
    if !ignore_error {
        for check in foreign_key_checks {
            runtime.foreign_key_update_check(check, &old_data, &new_data)?;
        }
    }
    for cascade in foreign_key_cascades {
        runtime.foreign_key_update_cascade(cascade, &old_data, &new_data)?;
    }
    // ON DUPLICATE KEY UPDATE 成功时 MySQL 计 2 行 affected。
    runtime.add_affected_rows(if on_duplicate { 2 } else { 1 });
    runtime.add_updated_rows(1);
    runtime.add_copied_rows(1);
    Ok((true, false))
}

/// 悲观事务中把未变更行的指定键集合加入待加锁列表。
pub fn addUnchangedKeysForLockByRow<R: WriteRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    handle: &R::Handle,
    row: &[R::Datum],
    key_set: u8,
) -> Result<usize, R::Error> {
    // 非悲观事务或未请求任何键时无需加锁。
    if !runtime.in_pessimistic_transaction() || key_set == 0 {
        return Ok(0);
    }
    runtime.add_unchanged_keys_for_lock(context, handle, row, key_set)
}

/// 主键写入 AUTO_RANDOM 值时，按掩码回填分配水位。
pub fn rebaseAutoRandomValue<R: WriteRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    new_data: &R::Datum,
    column: &R::Column,
) -> Result<(), R::Error> {
    if !runtime.table_contains_auto_random_bits() {
        return Ok(());
    }
    let mut record_id = runtime.auto_record_id(column, new_data)?;
    // 负 id 表示无需回填。
    if record_id < 0 {
        return Ok(());
    }
    record_id &= runtime.auto_random_incremental_mask(column);
    runtime.rebase_auto_random(context, record_id)
}

/// 列值过长错误（对应 MySQL Data too long for column）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataTooLongError {
    /// 出错列名。
    pub column_name: String,
    /// 出错行号（从 1 或会话约定起算，与 Go 一致传入）。
    pub row_index: usize,
}

impl fmt::Display for DataTooLongError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Data too long for column '{}' at row {}",
            self.column_name, self.row_index
        )
    }
}

impl std::error::Error for DataTooLongError {}

/// 构造「列值过长」错误对象。
pub fn resetErrDataTooLong(column_name: String, row_index: usize) -> DataTooLongError {
    DataTooLongError {
        column_name,
        row_index,
    }
}

/// 交换分区场景下校验行是否落在目标分区。
pub fn checkRowForExchangePartition<R: WriteRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    row: &[R::Datum],
) -> Result<(), R::Error> {
    runtime.check_exchange_partition_row(context, row)
}
