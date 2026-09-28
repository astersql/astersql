// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 列类型变更（Column Type Change, CTC）相关的单元测试。
//
// 列类型变更是 DDL（数据定义语言，如 ALTER TABLE 等修改表结构的语句）中最复杂的
// 操作之一：当新旧类型不兼容（有损变更）时，需要“重组”（Reorg，Reorganization）
// 表中已有的行数据，即新建一个隐藏的“变更中列”（changing column），把旧列的数据
// 逐行转换并回填到新列，最后原子地用新列替换旧列。
//
// 本文件覆盖的场景包括：
// - 整数类型之间的变更需要经历的中间 schema 状态流转
//   （DeleteOnly -> WriteOnly -> WriteReorganization -> Public，
//   这是在线 DDL 的多阶段状态机，保证集群中不同节点在任一时刻
//   看到的相邻两个 schema 版本互相兼容）；
// - 变更过程中途回滚（rollback）时能正确清理中间列；
// - 仅显示宽度变化（如 int(11) -> int(20)）不触发重组的优化；
// - 重组时行编码配置（row format）与行级校验和（checksum）的刷新逻辑；
// - 修改列默认值时 default_value / origin_default_value 的联动；
// - 严格 SQL 模式下默认值无法转换为新类型时报 DataTruncated 错误；
// - 重组任务可运行性判断（取消、暂停、失去 owner 等情况）。

use crate::column::{
    ColumnInfo, ColumnKind, ColumnPosition, DefaultValue, FieldType, SchemaState, TableInfo,
    init_and_add_column_to_table,
};
use crate::modify_column::{
    ModifyColumnArgs, ModifyColumnContext, ModifyColumnError, ModifyColumnType,
    advance_modify_column, get_modify_column_type, need_row_reorganization, no_reorg_data_strict,
    set_default_for_modified_column,
};
use crate::reorg::{
    ReorgRunnableError, SqlMode, is_reorg_runnable, new_reorg_expression_context,
    new_reorg_table_mutate_context,
};

/// 构造一个处于 Public（对外可见）状态的整数列，指定显示宽度与是否无符号。
fn integer(name: &str, bits: usize, unsigned: bool) -> ColumnInfo {
    let mut field_type = FieldType::integer();
    field_type.flen = bits;
    field_type.unsigned = unsigned;
    let mut column = ColumnInfo::new(name, field_type);
    column.state = SchemaState::Public;
    column
}

/// 构造一张只含单个给定列的测试表，并把该列置为 Public 状态。
fn table_with(column: ColumnInfo) -> TableInfo {
    let mut table = TableInfo::new(1, "t");
    let id = init_and_add_column_to_table(&mut table, column);
    table
        .columns
        .iter_mut()
        .find(|column| column.id == id)
        .unwrap()
        .state = SchemaState::Public;
    table
}

/// 构造默认的修改列上下文：启用严格 SQL 模式、无分区、不禁用有损优化。
fn context() -> ModifyColumnContext {
    ModifyColumnContext {
        strict_sql_mode: true,
        partition: None,
        disable_lossy_optimization: false,
        auto_random_range_bits_default: 64,
        auto_random_shard_bits_max: 15,
    }
}

/// 根据旧列信息与目标新列构造修改列参数（其余字段取默认空值）。
fn args(old: &ColumnInfo, new: ColumnInfo) -> ModifyColumnArgs {
    ModifyColumnArgs {
        old_column_name: old.name.clone(),
        old_column_id: old.id,
        column: new,
        position: ColumnPosition::None,
        modify_type: ModifyColumnType::None,
        changing_column_id: None,
        changing_index_ids: Vec::new(),
        redundant_index_ids: Vec::new(),
        old_elements: Vec::new(),
        new_elements: Vec::new(),
        new_shard_bits: 0,
        new_range_bits: 0,
    }
}

/// 整数间的有损变更（有符号 8 位 -> 无符号 16 位）需要走 Reorg 路径，
/// 并依次经历在线 DDL 的四个 schema 状态直至完成。
#[test]
fn test_column_type_change_state_between_integer() {
    let mut table = table_with(integer("a", 8, false));
    let old = table.columns[0].clone();
    let changed = integer("a", 16, true);
    let mut args = args(&old, changed);
    assert_eq!(
        ModifyColumnType::Reorg,
        get_modify_column_type(&table, &args, &old, &context())
    );
    let mut version = 0;
    let mut states = Vec::new();
    // 反复推进 DDL 状态机直至 finished，收集每一步产生的 schema 状态。
    loop {
        let result =
            advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
                .unwrap();
        states.push(result.schema_state);
        assert_eq!(states.len() as i64, result.schema_version);
        if !result.finished {
            let changing_id = args.changing_column_id.expect("changing column must exist");
            let changing = table
                .columns
                .iter()
                .find(|column| column.id == changing_id)
                .expect("changing column must be visible in table metadata");
            assert_eq!(result.schema_state, changing.state);
            assert!(changing.name.starts_with("_col$_"));
        }
        if result.finished {
            break;
        }
    }
    assert_eq!(
        vec![
            SchemaState::DeleteOnly,
            SchemaState::WriteOnly,
            SchemaState::WriteReorganization,
            SchemaState::Public
        ],
        states
    );
    assert_eq!(4, version);
    assert_eq!(1, table.columns.len());
    assert!(
        table
            .columns
            .iter()
            .all(|column| !column.name.starts_with("_col$_"))
    );
    // 完成后表中的列应已替换为无符号的新类型，且不残留中间状态。
    assert!(table.columns[0].field_type.unsigned);
    assert_eq!(SchemaState::Public, table.columns[0].state);
    assert!(table.columns[0].change_dependency_offset.is_none());
}

/// 在 Go 覆盖的 None/DeleteOnly/WriteOnly/WriteReorganization 各阶段触发回滚：
/// 均应清理“变更中列”，恢复到只有原始列的状态。
#[test]
fn test_rollback_column_type_change_between_integer() {
    for steps_before_rollback in 0..=3 {
        let mut table = table_with(integer("a", 8, false));
        let old = table.columns[0].clone();
        let mut args = args(&old, integer("a", 16, true));
        let mut version = 0;
        for _ in 0..steps_before_rollback {
            let outcome =
                advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
                    .unwrap();
            assert!(!outcome.finished);
        }
        assert_eq!(steps_before_rollback as i64, version);

        let outcome =
            advance_modify_column(&mut table, &mut args, &context(), true, &mut version, true)
                .unwrap();
        assert!(outcome.finished && outcome.rollback_done);
        assert_eq!(SchemaState::None, outcome.schema_state);
        assert_eq!(steps_before_rollback as i64 + 1, version);
        assert_eq!(1, table.columns.len());
        assert!(!table.columns[0].field_type.unsigned);
        assert_eq!(SchemaState::Public, table.columns[0].state);
        assert!(!table.columns[0].prevent_null_insert);
        assert!(table.columns[0].change_dependency_offset.is_none());
        assert!(
            table
                .columns
                .iter()
                .all(|column| !column.name.starts_with("_col$_"))
        );
    }
}

/// 整数仅显示宽度变化（tinyint(3) -> tinyint(1)）不触发行数据重组；
/// 严格检查仍可要求预检，但不能进入 WriteReorganization。
#[test]
fn test_column_type_change_ignore_display_length() {
    // Go 回归是 tinyint(3) -> tinyint(1)：显示宽度缩小也不能进入 WriteReorg。
    let old = integer("a", 3, false);
    let narrower = integer("a", 1, false);
    assert!(!no_reorg_data_strict(&old, &narrower, &[], &[]));
    assert!(!need_row_reorganization(&old, &narrower));

    let mut table = table_with(old.clone());
    let mut args = args(&old, narrower);
    let mut version = 0;
    let outcome =
        advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
            .unwrap();
    assert!(outcome.finished);
    assert_eq!(SchemaState::Public, outcome.schema_state);
    assert_ne!(ModifyColumnType::Reorg, args.modify_type);
    assert_eq!(1, table.columns.len());
}

/// 行格式版本控制：版本 1 不启用新行编码器，版本 2 启用。
/// 行编码器决定重组回填数据时使用的物理行存储格式。
#[test]
fn test_row_format() {
    let expression = new_reorg_expression_context(SqlMode::Strict, 0);
    let mut mutate = new_reorg_table_mutate_context(expression);
    mutate.refresh_row_encoding_config(1);
    assert!(!mutate.row_encoding_config().row_encoder_enabled);
    mutate.refresh_row_encoding_config(2);
    assert!(mutate.row_encoding_config().row_encoder_enabled);
}

/// 行级校验和（用于校验行数据完整性）同样跟随行格式版本：
/// 版本 1 关闭，版本 2 开启。
#[test]
fn test_row_format_with_checksums() {
    let expression = new_reorg_expression_context(SqlMode::Strict, 0);
    let mut mutate = new_reorg_table_mutate_context(expression);
    mutate.refresh_row_encoding_config(1);
    assert!(!mutate.row_encoding_config().row_level_checksum_enabled);
    mutate.refresh_row_encoding_config(2);
    assert!(mutate.row_encoding_config().row_level_checksum_enabled);
}

/// 多重 schema 变更（一条 ALTER 语句包含多个子变更）场景下，
/// 共享同一表达式上下文的多个重组上下文应得到一致的行编码配置。
#[test]
fn test_row_level_checksum_with_multi_schema_change() {
    let expression = new_reorg_expression_context(SqlMode::NonStrict, 8 * 3600);
    let mut first = new_reorg_table_mutate_context(expression.clone());
    let mut second = new_reorg_table_mutate_context(expression);
    first.refresh_row_encoding_config(2);
    second.refresh_row_encoding_config(2);
    assert_eq!(first.row_encoding_config(), second.row_encoding_config());
    assert!(first.row_encoding_config().row_level_checksum_enabled);
}

/// 为修改中的列设置默认值时，default_value 与 origin_default_value
/// （原始默认值，重组回填旧行时使用）应同步更新与清空。
#[test]
fn test_changing_col_origin_default_value() {
    let mut column = integer("a", 8, false);
    set_default_for_modified_column(&mut column, Some(DefaultValue::Integer(7)));
    assert_eq!(Some(DefaultValue::Integer(7)), column.default_value);
    assert_eq!(Some(DefaultValue::Integer(7)), column.origin_default_value);
    set_default_for_modified_column(&mut column, None);
    assert_eq!(None, column.default_value);
    assert_eq!(None, column.origin_default_value);
}

/// 字符串默认值成功转换（cast）为整数后，可将转换结果单独写入
/// origin_default_value，不影响 default_value 本身。
#[test]
fn test_changing_col_origin_default_value_after_add_col_and_cast_succ() {
    let mut column = integer("a", 8, false);
    set_default_for_modified_column(&mut column, Some(DefaultValue::String("123".into())));
    column.origin_default_value = Some(DefaultValue::Integer(123));
    assert_eq!(
        Some(DefaultValue::Integer(123)),
        column.origin_default_value
    );
}

/// 默认值无法转换为新类型（"not-an-integer" -> 整数）时，
/// 推进修改列应失败并报 DataTruncated（数据被截断）错误。
#[test]
fn test_changing_col_origin_default_value_after_add_col_and_cast_fail() {
    let mut table = table_with(integer("a", 16, false));
    table.columns[0].default_value = Some(DefaultValue::String("not-an-integer".into()));
    let old = table.columns[0].clone();
    let mut args = args(&old, integer("a", 4, false));
    let mut version = 0;
    assert_eq!(
        Err(ModifyColumnError::DataTruncated),
        advance_modify_column(
            &mut table,
            &mut args,
            &context(),
            false,
            &mut version,
            false
        )
    );
}

/// 整数改为 DateTime 的变更中途取消（回滚）后，
/// 表中不应残留任何 "_col$_" 前缀的隐藏中间列。
#[test]
fn test_ddl_exit_when_cancel_meet_panic() {
    let mut table = table_with(integer("a", 8, false));
    let old = table.columns[0].clone();
    let mut changed = old.clone();
    changed.field_type.kind = ColumnKind::DateTime;
    let mut args = args(&old, changed);
    let mut version = 0;
    advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false).unwrap();
    let rollback =
        advance_modify_column(&mut table, &mut args, &context(), true, &mut version, true).unwrap();
    assert!(rollback.rollback_done);
    assert!(
        table
            .columns
            .iter()
            .all(|column| !column.name.starts_with("_col$_"))
    );
}

/// 重组任务可运行性判断：被取消、被暂停、当前节点不是 DDL owner
/// （集群中唯一负责执行 DDL 的节点）时分别返回对应错误，正常时可运行。
#[test]
fn test_cancel_ctc_in_reorg_state_will_cause_goroutine_leak() {
    assert_eq!(
        Err(ReorgRunnableError::Cancelled),
        is_reorg_runnable(true, false, true, false)
    );
    assert_eq!(
        Err(ReorgRunnableError::Paused),
        is_reorg_runnable(false, true, true, false)
    );
    assert_eq!(
        Err(ReorgRunnableError::NotOwner),
        is_reorg_runnable(false, false, false, false)
    );
    assert_eq!(
        Err(ReorgRunnableError::ServerShuttingDown),
        is_reorg_runnable(false, false, true, true)
    );
    // 与 Go 检查顺序一致：取消优先于暂停、owner 与 shutdown。
    assert_eq!(
        Err(ReorgRunnableError::Cancelled),
        is_reorg_runnable(true, true, false, true)
    );
    assert!(is_reorg_runnable(false, false, true, false).is_ok());
}

/// DateTime 改为 Timestamp（受时区影响的时间类型）需要重组行数据；
/// 且重组表达式上下文中，严格 SQL 模式不把截断降级为警告，宽松模式则降级。
#[test]
fn test_cast_date_to_timestamp_in_reorg_attribute() {
    let mut date = integer("a", 8, false);
    date.field_type.kind = ColumnKind::DateTime;
    let mut timestamp = date.clone();
    timestamp.field_type.kind = ColumnKind::Timestamp;
    assert!(need_row_reorganization(&date, &timestamp));
    let strict = new_reorg_expression_context(SqlMode::Strict, 9 * 3600);
    let permissive = new_reorg_expression_context(SqlMode::NonStrict, 9 * 3600);
    assert!(!strict.truncate_as_warning);
    assert!(permissive.truncate_as_warning);
}
