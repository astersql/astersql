// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DDL（数据定义语言，如 CREATE/ALTER/DROP）执行过程中 schema 状态变更的测试集。
//
// 本模块验证在线 schema 变更（Online DDL）的核心状态机：列或索引的元数据
// 需要按 None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public 的
// 顺序逐步推进（即 F1/Google 提出的多阶段 schema 演进协议），从而保证集群中
// 不同节点即使看到相邻两个版本的 schema，数据也保持一致。
// 测试覆盖：加列/删列、修改列类型（含数据重组 reorg）、创建/删除各类索引
// （普通、唯一、主键、表达式、向量、倒排）、表的重命名/截断/删除，以及
// 多线程并发执行 DDL 时的元数据一致性与 DDL 任务（Job）的暂停/恢复。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::column::{
    ColumnError, ColumnInfo, ColumnKind, ColumnPosition, DefaultValue, FieldType,
    IndexColumn as ColumnIndexColumn, IndexInfo as ColumnIndexInfo, SchemaState,
    TableInfo as ColumnTable, ensure_column_droppable, init_and_add_column_to_table,
    remove_column_and_single_indices,
};
use crate::ddl::{
    AdminCommandOperator, Ddl, Job, JobCommand, JobState, OnExist, create_table_config,
};
use crate::index::{
    ColumnInfo as IndexColumnInfo, ColumnType, IndexError, IndexInfo, IndexKind, IndexOptions,
    TableInfo as IndexTable, build_index_info, remove_index_info, rename_expression_index_columns,
    rename_index, set_index_visibility,
};
use crate::modify_column::{
    ModifyColumnArgs, ModifyColumnContext, ModifyColumnError, ModifyColumnType,
    advance_modify_column, set_default_for_modified_column,
};
use crate::table::{TableCatalog, TableError, TableInfo, TableState, alter_charset_and_collation};

/// 构造一个指定名称的整数类型列定义（`ColumnInfo`），作为各测试的基础列构造器。
fn integer(name: &str) -> ColumnInfo {
    ColumnInfo::new(name, FieldType::integer())
}

/// 构造包含给定列名的测试表，并把每个列直接推进到 `Public`（公开可见）状态，
/// 模拟一张已完成 DDL、可正常读写的表。
fn column_table(names: &[&str]) -> ColumnTable {
    let mut table = ColumnTable::new(1, "t");
    for name in names {
        let id = init_and_add_column_to_table(&mut table, integer(name));
        table
            .columns
            .iter_mut()
            .find(|column| column.id == id)
            .unwrap()
            .state = SchemaState::Public;
    }
    table
}

/// 构造一条处于 `Public` 状态的索引元数据（`ColumnIndexInfo`），
/// `primary` 表示是否为主键索引；列偏移等字段使用测试用的固定值。
fn column_index(id: i64, name: &str, columns: &[&str], primary: bool) -> ColumnIndexInfo {
    ColumnIndexInfo {
        id,
        name: name.into(),
        state: SchemaState::Public,
        columns: columns
            .iter()
            .map(|name| ColumnIndexColumn {
                name: (*name).into(),
                offset: 0,
                length: None,
                use_changing_type: false,
            })
            .collect(),
        primary,
        columnar: false,
    }
}

/// 构造“修改列”操作所需的上下文：启用严格 SQL 模式、无分区，
/// 并给出 auto_random（随机自增主键）相关的位数默认值与上限。
fn modify_context() -> ModifyColumnContext {
    ModifyColumnContext {
        strict_sql_mode: true,
        partition: None,
        disable_lossy_optimization: false,
        auto_random_range_bits_default: 64,
        auto_random_shard_bits_max: 15,
    }
}

/// 根据旧列与新列定义构造修改列的参数集（`ModifyColumnArgs`），
/// 其余字段（列位置、变更中间列、关联索引等）均取默认空值。
fn modify_args(old: &ColumnInfo, new: ColumnInfo) -> ModifyColumnArgs {
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

/// 驱动一次完整的“修改列类型”状态机流转并收集每步的 schema 状态。
///
/// reorg（reorganization，数据重组）指列类型变更需要重写已有行数据的过程；
/// `indexed` 控制该列是否带索引，`data_is_valid` 模拟存量数据能否转换为新类型。
/// 返回最终表结构、途经的状态序列以及 schema 版本号。
fn run_reorg(
    old_kind: ColumnKind,
    new_kind: ColumnKind,
    indexed: bool,
    data_is_valid: bool,
) -> Result<(ColumnTable, Vec<SchemaState>, i64), ModifyColumnError> {
    let mut table = column_table(&["a", "b"]);
    table.columns[0].field_type.kind = old_kind;
    if indexed {
        table.indices.push(column_index(10, "idx_a", &["a"], false));
    }
    let old = table.columns[0].clone();
    let mut new = old.clone();
    new.field_type.kind = new_kind;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    let mut states = Vec::new();
    loop {
        let outcome = advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            data_is_valid,
            &mut version,
            false,
        )?;
        states.push(outcome.schema_state);
        if outcome.finished {
            break;
        }
    }
    Ok((table, states, version))
}

/// 断言列类型变更完整经历 DeleteOnly -> WriteOnly -> WriteReorganization -> Public
/// 四个阶段、schema 版本递增 4 次，且过程中产生的临时“changing”列已被清理。
fn assert_reorg(old_kind: ColumnKind, new_kind: ColumnKind, indexed: bool) {
    let (table, states, version) = run_reorg(old_kind, new_kind, indexed, true).unwrap();
    assert_eq!(
        vec![
            SchemaState::DeleteOnly,
            SchemaState::WriteOnly,
            SchemaState::WriteReorganization,
            SchemaState::Public,
        ],
        states
    );
    assert_eq!(4, version);
    assert_eq!(new_kind, table.columns[0].field_type.kind);
    assert!(
        table
            .columns
            .iter()
            .all(|column| !column.name.starts_with("_tidb_changing_"))
    );
}

/// 验证向表中新增一列并置为指定 schema 状态后，列状态与默认值均按预期保留。
fn assert_write_state(state: SchemaState, default: Option<DefaultValue>) {
    let mut table = column_table(&["a"]);
    let mut column = integer("b");
    column.default_value = default.clone();
    let id = init_and_add_column_to_table(&mut table, column);
    let added = table
        .columns
        .iter_mut()
        .find(|column| column.id == id)
        .unwrap();
    added.state = state;
    assert_eq!(state, added.state);
    assert_eq!(default, added.default_value);
}

/// 构造索引模块使用的列信息（`IndexColumnInfo`）；`hidden` 为 true 时
/// 表示这是表达式索引依赖的隐藏生成列（用户不可见，由表达式计算得到）。
fn index_column(name: &str, column_type: ColumnType, hidden: bool) -> IndexColumnInfo {
    IndexColumnInfo {
        id: 1,
        name: name.into(),
        column_type,
        charset_max_bytes: 4,
        generated: hidden,
        stored: hidden,
        hidden,
        nullable: true,
        primary_key: false,
        index_flags: 0,
        generated_dependencies: BTreeSet::new(),
    }
}

/// 构造含 `a`(Int) 与 `b`(VarChar(32)) 两列、无索引的索引测试表。
fn index_table() -> IndexTable {
    IndexTable {
        id: 1,
        columns: vec![
            index_column("a", ColumnType::Int, false),
            index_column("b", ColumnType::VarChar(32), false),
        ],
        indices: Vec::new(),
        max_index_id: 0,
        partitioned: false,
    }
}

/// 在表上为单个列构建一条使用默认选项的普通二级索引。
fn add_index(table: &mut IndexTable, name: &str, column: &str) -> IndexInfo {
    build_index_info(
        table,
        name,
        &[(column.into(), None)],
        IndexOptions::default(),
    )
    .unwrap()
}

/// 构造表目录（catalog，数据库的元数据注册表）中的一条表元数据记录，
/// 状态为 `Public`，字符集默认 utf8mb4。
fn catalog_table(id: i64, schema_id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        schema_id,
        name: name.into(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// 构造一个 DDL 任务（Job）：DDL 语句在内部被封装为任务，
/// 由 DDL 调度器（owner）串行或并行地推进执行。
fn job(id: i64, query: &str) -> Job {
    Job {
        id,
        query: query.into(),
        state: JobState::None,
        version: 0,
        start_ts: id as u64,
        real_start_ts: 0,
        action_type: crate::ddl::ActionType::Other,
        table_id: 1,
        schema_id: 1,
        paused_by: None,
    }
}

/// 构造一个已启动（started = true）的 DDL 调度器实例。
fn running_ddl() -> Ddl {
    let mut ddl = Ddl::new("db-change-test", Vec::new());
    ddl.started = true;
    ddl
}

/// 按在线 DDL 的三步状态流转删除表：WriteOnly -> DeleteOnly -> None，
/// 逐步收窄可见性以保证并发事务安全。
fn drop_table(catalog: &mut TableCatalog, schema_id: i64, name: &str) {
    assert_eq!(
        TableState::WriteOnly,
        catalog.drop_table_step(schema_id, name, 100).unwrap()
    );
    assert_eq!(
        TableState::DeleteOnly,
        catalog.drop_table_step(schema_id, name, 100).unwrap()
    );
    assert_eq!(
        TableState::None,
        catalog.drop_table_step(schema_id, name, 100).unwrap()
    );
}

/// 验证建表后的元数据：新增列初始为 `None` 状态，需显式推进到 `Public` 才可见，
/// 同时检查默认字符集/排序规则。
#[test]
fn test_show_create_table() {
    let mut table = column_table(&["id"]);
    table.indices.push(column_index(1, "idx", &["id"], false));
    let id = init_and_add_column_to_table(&mut table, integer("c"));
    assert_eq!("utf8mb4", table.charset);
    assert_eq!("utf8mb4_bin", table.collation);
    assert_eq!(
        SchemaState::None,
        table.columns.iter().find(|c| c.id == id).unwrap().state
    );
    table.columns.iter_mut().find(|c| c.id == id).unwrap().state = SchemaState::Public;
    assert_eq!(
        vec!["id", "c"],
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 验证删除带 NOT NULL 约束和默认值的列：列被移除且无关联索引需要一并删除。
#[test]
fn test_drop_not_null_column() {
    let mut table = column_table(&["id", "a"]);
    table.columns[1].not_null = true;
    table.columns[1].default_value = Some(DefaultValue::Integer(11));
    let id = table.columns[1].id;
    assert!(
        remove_column_and_single_indices(&mut table, id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        vec!["id"],
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
}

// Only the callback boundary is substituted here: column creation, state
// advancement, defaults and AFTER positioning use the real add-column module.
// The SQL compiler/executor coverage of the older Go suite is independent of
// this commit's callback scheduling change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StateCheckStep {
    Compile(usize),
    Execute(usize),
}

#[derive(Default)]
struct TwoStateChecks {
    previous: SchemaState,
    visits: usize,
    error: Option<&'static str>,
}

impl TwoStateChecks {
    fn observe(
        &mut self,
        state: SchemaState,
        mut check: impl FnMut(StateCheckStep) -> Result<(), &'static str>,
    ) {
        // Retain the later target-state filter from 0079af820ee15bc88759027fcf16dadba5dbfb28.
        if !matches!(
            state,
            SchemaState::DeleteOnly | SchemaState::WriteOnly | SchemaState::WriteReorganization
        ) || state == self.previous
            || self.error.is_some()
            || self.visits >= 3
        {
            return;
        }
        self.previous = state;
        self.visits += 1;
        let steps: &[StateCheckStep] = match state {
            SchemaState::DeleteOnly => &[StateCheckStep::Compile(0), StateCheckStep::Execute(0)],
            SchemaState::WriteOnly => &[StateCheckStep::Compile(1)],
            SchemaState::WriteReorganization => &[
                StateCheckStep::Compile(2),
                StateCheckStep::Execute(2),
                StateCheckStep::Execute(1),
                StateCheckStep::Compile(3),
            ],
            _ => unreachable!(),
        };
        for step in steps {
            if let Err(error) = check(*step) {
                self.error = Some(error);
                break;
            }
        }
    }
}

fn fallback_state_checks(
    mut check: impl FnMut(StateCheckStep) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    for step in [
        StateCheckStep::Compile(0),
        StateCheckStep::Execute(0),
        StateCheckStep::Compile(1),
        StateCheckStep::Compile(2),
        StateCheckStep::Execute(2),
        StateCheckStep::Execute(1),
        StateCheckStep::Compile(3),
    ] {
        check(step)?;
    }
    Ok(())
}

fn two_states_column(table: &mut ColumnTable) -> i64 {
    use crate::add_column::{ColumnConstraint, ColumnDefinition, start_add_column};
    let mut field_type = FieldType::integer();
    field_type.kind = ColumnKind::Enum;
    start_add_column(
        table,
        &ColumnDefinition {
            name: "d3".into(),
            field_type,
            constraints: vec![ColumnConstraint::NotNull],
            default_value: Some(DefaultValue::String("a".into())),
            comment: String::new(),
            generated: None,
        },
        &ColumnPosition::After("c3".into()),
        512,
        false,
        false,
    )
    .unwrap()
}

#[test]
fn test_two_states() {
    use crate::add_column::advance_add_column;
    // Probe a real transition with a callback present and absent, matching the
    // rewritten-hook/plain-test distinction without changing global failpoints.
    for hook_enabled in [true, false] {
        let mut probe = column_table(&["a"]);
        let probe_id = two_states_column(&mut probe);
        let mut probe_version = 0;
        let probe_outcome = advance_add_column(
            &mut probe,
            probe_id,
            &ColumnPosition::None,
            &mut probe_version,
            false,
        )
        .unwrap();
        let hook_name = if hook_enabled {
            "ddl-two-states-rewritten"
        } else {
            "ddl-two-states-marker"
        };
        let observations = Arc::new(Mutex::new(Vec::new()));
        let probe_observations = Arc::clone(&observations);
        let probe_guard =
            astersql_testkit_testfailpoint::enable_value_call(hook_name, move |value| {
                probe_observations.lock().unwrap().push(value.to_owned());
            });
        // A plain-test marker does not reach the registered callback. Only
        // this external injection boundary differs between the two modes.
        if hook_enabled {
            astersql_testkit_testfailpoint::inject_value(
                hook_name,
                &format!("{:?}", probe_outcome.schema_state),
            );
        }
        let hook_available = !observations.lock().unwrap().is_empty();
        assert_eq!(hook_enabled, hook_available);
        drop(probe_guard);
        observations.lock().unwrap().clear();
        astersql_testkit_testfailpoint::inject_value(hook_name, "Public");
        assert!(
            observations.lock().unwrap().is_empty(),
            "probe callback must be disabled before the target DDL"
        );
        drop(probe);
        let target_observations = Arc::clone(&observations);
        let target_guard =
            astersql_testkit_testfailpoint::enable_value_call(hook_name, move |value| {
                target_observations.lock().unwrap().push(value.to_owned());
            });

        let mut table = column_table(&["c1", "c2", "c3", "c4"]);
        let id = two_states_column(&mut table);
        let position = ColumnPosition::After("c3".into());
        let mut version = 0;
        let mut checks = TwoStateChecks::default();
        let mut steps = Vec::new();
        let mut snapshots = BTreeMap::new();
        let mut check = |step| {
            if let StateCheckStep::Compile(case) = step {
                snapshots.insert(case, table.clone());
            } else if let StateCheckStep::Execute(case) = step {
                assert!(snapshots.contains_key(&case), "execute must follow compile");
            }
            steps.push(step);
            Ok(())
        };
        if !hook_available {
            fallback_state_checks(&mut check).unwrap();
        }
        drop(check);
        loop {
            let outcome =
                advance_add_column(&mut table, id, &position, &mut version, false).unwrap();
            if hook_available {
                // Replay the same real observation, as schema synchronization
                // can report a state more than once. Do not advance the state.
                for _ in 0..2 {
                    astersql_testkit_testfailpoint::inject_value(
                        hook_name,
                        &format!("{:?}", outcome.schema_state),
                    );
                }
                for observed in observations.lock().unwrap().drain(..) {
                    let state = match observed.as_str() {
                        "DeleteOnly" => SchemaState::DeleteOnly,
                        "WriteOnly" => SchemaState::WriteOnly,
                        "WriteReorganization" => SchemaState::WriteReorganization,
                        "Public" => SchemaState::Public,
                        _ => panic!("unexpected production state {observed}"),
                    };
                    checks.observe(state, |step| {
                        if let StateCheckStep::Compile(case) = step {
                            snapshots.insert(case, table.clone());
                        } else if let StateCheckStep::Execute(case) = step {
                            assert!(snapshots.contains_key(&case));
                        }
                        steps.push(step);
                        Ok(())
                    });
                }
            }
            if outcome.finished {
                break;
            }
        }
        drop(target_guard);
        assert_eq!(if hook_available { 3 } else { 0 }, checks.visits);
        assert_eq!(None, checks.error);
        assert_eq!(
            vec![
                StateCheckStep::Compile(0),
                StateCheckStep::Execute(0),
                StateCheckStep::Compile(1),
                StateCheckStep::Compile(2),
                StateCheckStep::Execute(2),
                StateCheckStep::Execute(1),
                StateCheckStep::Compile(3),
            ],
            steps
        );
        assert_eq!(4, version);
        assert_eq!(
            vec!["c1", "c2", "c3", "d3", "c4"],
            table
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
        );
        for (case, snapshot) in snapshots {
            let column = snapshot.columns.iter().find(|c| c.id == id).unwrap();
            assert_ne!(SchemaState::Public, column.state);
            assert_eq!(Some(DefaultValue::String("a".into())), column.default_value);
            if hook_available {
                assert_eq!(
                    match case {
                        0 => SchemaState::DeleteOnly,
                        1 => SchemaState::WriteOnly,
                        _ => SchemaState::WriteReorganization,
                    },
                    column.state
                );
            } else {
                assert_eq!(SchemaState::None, column.state);
            }
        }
    }
}

#[test]
fn test_two_states_stops_after_check_error() {
    let mut table = column_table(&["c1", "c2", "c3", "c4"]);
    let id = two_states_column(&mut table);
    let mut version = 0;
    let outcome = crate::add_column::advance_add_column(
        &mut table,
        id,
        &ColumnPosition::After("c3".into()),
        &mut version,
        false,
    )
    .unwrap();
    let mut checks = TwoStateChecks::default();
    let mut called = Vec::new();
    checks.observe(outcome.schema_state, |step| {
        called.push(step);
        Err("compile failed")
    });
    let outcome = crate::add_column::advance_add_column(
        &mut table,
        id,
        &ColumnPosition::After("c3".into()),
        &mut version,
        false,
    )
    .unwrap();
    checks.observe(outcome.schema_state, |_| {
        panic!("must preserve first error")
    });
    assert_eq!(Some("compile failed"), checks.error);
    assert_eq!(1, checks.visits);
    assert_eq!(vec![StateCheckStep::Compile(0)], called);
    let mut fallback_calls = 0;
    assert_eq!(
        Err("compile failed"),
        fallback_state_checks(|_| {
            fallback_calls += 1;
            Err("compile failed")
        })
    );
    assert_eq!(1, fallback_calls);
}

/// 验证 WriteOnly 状态下新增列可携带 NULL 默认值。
#[test]
fn test_write_only_write_null() {
    assert_write_state(SchemaState::WriteOnly, Some(DefaultValue::Null));
}

/// 验证 WriteOnly 状态下新增列保留整数默认值（对应 ON DUPLICATE KEY UPDATE 场景）。
#[test]
fn test_write_only_on_dup_update() {
    assert_write_state(SchemaState::WriteOnly, Some(DefaultValue::Integer(2)));
}

/// 验证一次加多列时，每个新列都能停留在 WriteOnly 状态并保留各自默认值。
#[test]
fn test_write_only_on_dup_update_for_add_columns() {
    let mut table = column_table(&["a"]);
    for (name, value) in [("b", 2), ("c", 3)] {
        let mut column = integer(name);
        column.state = SchemaState::WriteOnly;
        column.default_value = Some(DefaultValue::Integer(value));
        let id = init_and_add_column_to_table(&mut table, column);
        let added = table
            .columns
            .iter_mut()
            .find(|column| column.id == id)
            .unwrap();
        added.state = SchemaState::WriteOnly;
        assert_eq!(Some(DefaultValue::Integer(value)), added.default_value);
    }
}

/// 验证 Timestamp -> Integer 的列类型变更需要走完整的数据重组（reorg）流程。
#[test]
fn test_write_reorg_for_modify_column_timestamp_to_int() {
    assert_reorg(ColumnKind::Timestamp, ColumnKind::Integer, false);
}

/// 验证 Integer -> DateTime 的列类型变更走完整 reorg 流程。
#[test]
fn test_write_reorg_for_modify_column() {
    assert_reorg(ColumnKind::Integer, ColumnKind::DateTime, false);
}

/// 验证带索引的列做类型变更时 reorg 流程同样正确（索引需随数据一起重建）。
#[test]
fn test_write_reorg_for_modify_column_with_uniq_idx() {
    assert_reorg(ColumnKind::Integer, ColumnKind::DateTime, true);
}

/// 验证主键列（pk is handle：主键即行记录的物理句柄）做类型变更后，
/// 主键索引仍保持 primary 标记且列名不变。
#[test]
fn test_write_reorg_for_modify_column_with_pk_is_handle() {
    let mut table = column_table(&["a", "b"]);
    table
        .indices
        .push(column_index(10, "primary", &["a"], true));
    let old = table.columns[0].clone();
    let mut new = old.clone();
    new.field_type.kind = ColumnKind::DateTime;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    while !advance_modify_column(
        &mut table,
        &mut args,
        &modify_context(),
        true,
        &mut version,
        false,
    )
    .unwrap()
    .finished
    {}
    assert_eq!(4, version);
    assert!(table.indices[0].primary);
    assert_eq!("a", table.indices[0].columns[0].name);
}

/// 主键索引场景与 pk-is-handle 场景在本模型中行为一致，复用其断言。
#[test]
fn test_write_reorg_for_modify_column_with_primary_idx() {
    test_write_reorg_for_modify_column_with_pk_is_handle();
}

/// 验证修改中间列（未指定 FIRST/AFTER 位置）后各列顺序保持不变。
#[test]
fn test_write_reorg_for_modify_column_without_first() {
    let mut table = column_table(&["a", "b", "c"]);
    let old = table.columns[1].clone();
    let mut new = old.clone();
    new.field_type.kind = ColumnKind::DateTime;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    while !advance_modify_column(
        &mut table,
        &mut args,
        &modify_context(),
        true,
        &mut version,
        false,
    )
    .unwrap()
    .finished
    {}
    assert_eq!(
        vec!["a", "b", "c"],
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 验证无默认值的列经过 reorg 后默认值与原始默认值均保持为空。
#[test]
fn test_write_reorg_for_modify_column_without_default_val() {
    let (table, _, _) = run_reorg(ColumnKind::Integer, ColumnKind::DateTime, false, true).unwrap();
    assert_eq!(None, table.columns[0].default_value);
    assert_eq!(None, table.columns[0].origin_default_value);
}

/// 验证修改列流程第一步进入 DeleteOnly 状态时不会给列引入默认值。
#[test]
fn test_delete_only_for_modify_column_without_default_val() {
    let mut table = column_table(&["a"]);
    let old = table.columns[0].clone();
    let mut new = old.clone();
    new.field_type.kind = ColumnKind::DateTime;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    let outcome = advance_modify_column(
        &mut table,
        &mut args,
        &modify_context(),
        true,
        &mut version,
        false,
    )
    .unwrap();
    assert_eq!(SchemaState::DeleteOnly, outcome.schema_state);
    assert_eq!(None, table.columns[0].default_value);
}

/// 验证 WriteOnly 状态下新增列保留默认值。
#[test]
fn test_write_only() {
    assert_write_state(SchemaState::WriteOnly, Some(DefaultValue::Integer(3)));
}

/// 一次加多列的 WriteOnly 行为与逐列场景一致，复用其断言。
#[test]
fn test_write_only_for_add_columns() {
    test_write_only_on_dup_update_for_add_columns();
}

/// 验证 DeleteOnly 状态下新增列保留默认值。
#[test]
fn test_delete_only() {
    assert_write_state(SchemaState::DeleteOnly, Some(DefaultValue::Integer(3)));
}

/// 验证删除列时，仅覆盖该列的单列索引会被一并删除并返回其索引 ID。
#[test]
fn test_schema_change_for_drop_column_with_indexes() {
    let mut table = column_table(&["a", "b"]);
    let b = table.columns[1].id;
    table.indices.push(column_index(10, "idx_b", &["b"], false));
    assert_eq!(
        vec![10],
        remove_column_and_single_indices(&mut table, b).unwrap()
    );
    assert!(table.indices.is_empty());
}

/// 验证依次删除多个带索引的列时，各自的单列索引也被逐一删除。
#[test]
fn test_schema_change_for_drop_columns_with_indexes() {
    let mut table = column_table(&["a", "b", "c"]);
    table.indices.push(column_index(10, "idx_b", &["b"], false));
    table.indices.push(column_index(11, "idx_c", &["c"], false));
    let b = table.columns.iter().find(|c| c.name == "b").unwrap().id;
    let c = table.columns.iter().find(|c| c.name == "c").unwrap().id;
    assert_eq!(
        vec![10],
        remove_column_and_single_indices(&mut table, b).unwrap()
    );
    assert_eq!(
        vec![11],
        remove_column_and_single_indices(&mut table, c).unwrap()
    );
    assert_eq!(1, table.columns.len());
}

/// 验证删除表达式索引（基于表达式而非普通列的索引）时，
/// 其依赖的隐藏生成列也会被清理。
#[test]
fn test_delete_only_for_drop_expression_index() {
    let mut table = index_table();
    let mut hidden = index_column("_V$_expr_0", ColumnType::Int, true);
    hidden.generated_dependencies.insert("a".into());
    table.columns.push(hidden);
    add_index(&mut table, "expr", "_V$_expr_0");
    remove_index_info(&mut table, "expr").unwrap();
    assert!(table.columns.iter().all(|column| !column.hidden));
}

/// 验证按逆序删除多列后，表中仅剩余未删除的列。
#[test]
fn test_delete_only_for_drop_columns() {
    let mut table = column_table(&["a", "b", "c"]);
    let ids = [table.columns[2].id, table.columns[1].id];
    for id in ids {
        remove_column_and_single_indices(&mut table, id).unwrap();
    }
    assert_eq!(
        vec!["a"],
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 验证处于 WriteOnly 状态的列仍可通过删除前的可删性检查。
#[test]
fn test_write_only_for_drop_column() {
    let mut table = column_table(&["a", "b"]);
    table.columns[1].state = SchemaState::WriteOnly;
    assert_eq!(SchemaState::WriteOnly, table.columns[1].state);
    assert_eq!(Ok(()), ensure_column_droppable(&table, "b"));
}

/// 验证多列可同时处于 WriteOnly 删除中间状态。
#[test]
fn test_write_only_for_drop_columns() {
    let mut table = column_table(&["a", "b", "c"]);
    table.columns[1].state = SchemaState::WriteOnly;
    table.columns[2].state = SchemaState::WriteOnly;
    assert!(
        table.columns[1..]
            .iter()
            .all(|column| column.state == SchemaState::WriteOnly)
    );
}

/// 验证新建索引的 ID/名称，以及将索引设为不可见（invisible，优化器忽略但仍维护）。
#[test]
fn test_show_index() {
    let mut table = index_table();
    let index = add_index(&mut table, "idx_a", "a");
    assert_eq!(1, index.id);
    assert_eq!("idx_a", table.indices[0].name);
    set_index_visibility(&mut table, "idx_a", true).unwrap();
    assert!(table.indices[0].invisible);
}

/// 验证两个线程并发添加索引时，互斥锁保证索引数量与最大索引 ID 均正确。
#[test]
fn test_parallel_alter_index() {
    let table = Arc::new(Mutex::new(index_table()));
    let handles: Vec<_> = [("idx_a", "a"), ("idx_b", "b")]
        .into_iter()
        .map(|(name, column)| {
            let table = Arc::clone(&table);
            thread::spawn(move || add_index(&mut table.lock().unwrap(), name, column))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let table = table.lock().unwrap();
    assert_eq!(2, table.indices.len());
    assert_eq!(2, table.max_index_id);
}

/// 验证两个线程并发修改不同列的类型，最终两列类型均按各自目标生效。
#[test]
fn test_parallel_alter_modify_column() {
    let table = Arc::new(Mutex::new(column_table(&["a", "b"])));
    let handles: Vec<_> = [("a", ColumnKind::DateTime), ("b", ColumnKind::Timestamp)]
        .into_iter()
        .map(|(name, kind)| {
            let table = Arc::clone(&table);
            thread::spawn(move || {
                let mut table = table.lock().unwrap();
                let old = table
                    .columns
                    .iter()
                    .find(|column| column.name == name)
                    .unwrap()
                    .clone();
                let mut new = old.clone();
                new.field_type.kind = kind;
                let mut args = modify_args(&old, new);
                let mut version = 0;
                while !advance_modify_column(
                    &mut table,
                    &mut args,
                    &modify_context(),
                    true,
                    &mut version,
                    false,
                )
                .unwrap()
                .finished
                {}
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let table = table.lock().unwrap();
    assert_eq!(ColumnKind::DateTime, table.columns[0].field_type.kind);
    assert_eq!(ColumnKind::Timestamp, table.columns[1].field_type.kind);
}

/// 带存量数据（索引列）的并发修改列场景，复用完整 reorg 断言。
#[test]
fn test_parallel_alter_modify_column_with_data() {
    assert_reorg(ColumnKind::Integer, ColumnKind::DateTime, true);
}

/// 验证存量数据含 NULL 时把列改为 NOT NULL 会报 `InvalidNull` 错误，
/// 且 schema 版本不会推进。
#[test]
fn test_parallel_alter_modify_column_to_not_null_with_data() {
    let mut table = column_table(&["a"]);
    let old = table.columns[0].clone();
    let mut new = old.clone();
    new.not_null = true;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    assert_eq!(
        Err(ModifyColumnError::InvalidNull),
        advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            false,
            &mut version,
            false
        )
    );
    assert_eq!(0, version);
}

/// 验证当存在依赖该列的生成列（generated column，值由表达式计算）时，
/// 修改被依赖列的类型会被拒绝。
#[test]
fn test_parallel_add_generated_column_and_alter_modify_column() {
    let mut table = column_table(&["a"]);
    let mut generated = integer("g");
    generated.generated = true;
    generated.generated_expression = "a + 1".into();
    generated.dependencies.insert("a".into());
    init_and_add_column_to_table(&mut table, generated);
    let old = table.columns[0].clone();
    let mut new = old.clone();
    new.field_type.kind = ColumnKind::DateTime;
    let mut args = modify_args(&old, new);
    let mut version = 0;
    assert_eq!(
        Err(ModifyColumnError::DependentGeneratedColumn("g".into())),
        advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            true,
            &mut version,
            false
        )
    );
}

/// 验证添加主键索引：主键隐含唯一性，且对应列会被打上索引标记。
#[test]
fn test_parallel_alter_modify_column_and_add_pk() {
    let mut table = index_table();
    let primary = build_index_info(
        &mut table,
        "primary",
        &[("a".into(), None)],
        IndexOptions {
            primary: true,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    assert!(primary.primary && primary.unique);
    assert_eq!(1, table.columns[0].index_flags);
}

/// 验证新增列后设置默认值时，default_value 与 origin_default_value 保持同步。
#[test]
fn test_parallel_add_colum_and_set_default_value() {
    let mut table = column_table(&["a"]);
    let id = init_and_add_column_to_table(&mut table, integer("b"));
    let column = table
        .columns
        .iter_mut()
        .find(|column| column.id == id)
        .unwrap();
    set_default_for_modified_column(column, Some(DefaultValue::Integer(7)));
    assert_eq!(column.default_value, column.origin_default_value);
}

/// 验证仅重命名列（类型不变）可一步完成，无需 reorg，列顺序不变。
#[test]
fn test_parallel_change_column_name() {
    let mut table = column_table(&["a", "b"]);
    let old = table.columns[0].clone();
    let mut renamed = old.clone();
    renamed.name = "c".into();
    let mut args = modify_args(&old, renamed);
    let mut version = 0;
    assert!(
        advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            true,
            &mut version,
            false
        )
        .unwrap()
        .finished
    );
    assert_eq!(
        vec!["c", "b"],
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 并发加索引场景与 test_parallel_alter_index 相同，复用其断言。
#[test]
fn test_parallel_alter_add_index() {
    test_parallel_alter_index();
}

/// 验证在向量类型列上创建向量索引（用于近似最近邻检索）。
#[test]
fn test_parallel_alter_add_vector_index() {
    let mut table = index_table();
    table
        .columns
        .push(index_column("v", ColumnType::Vector, false));
    let index = build_index_info(
        &mut table,
        "vec_idx",
        &[("v".into(), None)],
        IndexOptions {
            kind: IndexKind::Vector,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    assert_eq!(IndexKind::Vector, index.kind);
}

/// 验证在整数列上创建倒排索引（Go 仅允许底层按整数存储的类型）。
#[test]
fn test_parallel_alter_add_columnar_index() {
    let mut table = index_table();
    table
        .columns
        .push(index_column("j", ColumnType::Int, false));
    let index = build_index_info(
        &mut table,
        "inv_idx",
        &[("j".into(), None)],
        IndexOptions {
            kind: IndexKind::Inverted,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    assert_eq!(IndexKind::Inverted, index.kind);
}

/// 验证基于隐藏生成列创建表达式索引，索引列名指向该隐藏列。
#[test]
fn test_parallel_alter_add_expression_index() {
    let mut table = index_table();
    let mut hidden = index_column("_V$_expr_0", ColumnType::Int, true);
    hidden.generated_dependencies.insert("a".into());
    table.columns.push(hidden);
    let index = add_index(&mut table, "expr", "_V$_expr_0");
    assert_eq!("_V$_expr_0", index.columns[0].name);
}

/// 并发加主键场景复用添加主键索引的断言。
#[test]
fn test_parallel_add_primary_key() {
    test_parallel_alter_modify_column_and_add_pk();
}

/// 验证在分区表上创建全局索引（global index，跨所有分区的单一索引），
/// 全局索引使用版本 2 的索引格式。
#[test]
fn test_parallel_alter_add_partition() {
    let mut table = index_table();
    table.partitioned = true;
    let index = build_index_info(
        &mut table,
        "global_a",
        &[("a".into(), None)],
        IndexOptions {
            global: true,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    assert!(index.global);
    assert_eq!(2, index.version);
}

/// 并发删列场景复用删除带索引列的断言。
#[test]
fn test_parallel_drop_column() {
    test_schema_change_for_drop_column_with_indexes();
}

/// 并发删多列场景复用相应断言。
#[test]
fn test_parallel_drop_columns() {
    test_schema_change_for_drop_columns_with_indexes();
}

/// 验证重复删除同一列时，第二次删除返回 `ColumnNotFound`（对应 IF EXISTS 语义）。
#[test]
fn test_parallel_drop_if_exists_columns() {
    let mut table = column_table(&["a", "b"]);
    let b = table.columns[1].id;
    assert!(remove_column_and_single_indices(&mut table, b).is_ok());
    assert_eq!(
        Err(ColumnError::ColumnNotFound(b.to_string())),
        remove_column_and_single_indices(&mut table, b)
    );
}

/// 验证重复删除同一索引时，第二次删除返回 `IndexNotFound`。
#[test]
fn test_parallel_drop_index() {
    let mut table = index_table();
    add_index(&mut table, "idx_a", "a");
    assert_eq!(
        "idx_a",
        remove_index_info(&mut table, "idx_a").unwrap().name
    );
    assert_eq!(
        Err(IndexError::IndexNotFound("idx_a".into())),
        remove_index_info(&mut table, "idx_a")
    );
}

/// 验证删除主键索引后，列上的索引标记被清除。
#[test]
fn test_parallel_drop_primary_key() {
    let mut table = index_table();
    build_index_info(
        &mut table,
        "primary",
        &[("a".into(), None)],
        IndexOptions {
            primary: true,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    let removed = remove_index_info(&mut table, "primary").unwrap();
    assert!(removed.primary);
    assert_eq!(0, table.columns[0].index_flags);
}

/// 验证表目录中的重命名：旧名不再可查，新名指向原表 ID。
#[test]
fn test_parallel_create_and_rename() {
    let mut catalog = TableCatalog::default();
    catalog.insert(catalog_table(1, 1, "t")).unwrap();
    catalog.rename_table(1, "t", 1, "t1").unwrap();
    assert_eq!(1, catalog.get(1, "t1").unwrap().id);
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "t"));
}

/// 验证先修改表字符集/排序规则、再按三步状态流转删除表后，表不再可查。
#[test]
fn test_parallel_alter_and_drop_schema() {
    let mut catalog = TableCatalog::default();
    catalog.insert(catalog_table(1, 1, "t")).unwrap();
    alter_charset_and_collation(
        catalog.get_mut(1, "t").unwrap(),
        "utf8mb4",
        "utf8mb4_general_ci",
    )
    .unwrap();
    drop_table(&mut catalog, 1, "t");
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "t"));
}

/// 验证 CREATE TABLE IF NOT EXISTS 的配置为 `OnExist::Ignore`，
/// 而目录层面重复插入同名表仍会报 `AlreadyExists`。
#[test]
fn test_create_table_if_not_exists() {
    let config = create_table_config(Some(OnExist::Ignore), false);
    let mut catalog = TableCatalog::default();
    catalog.insert(catalog_table(1, 1, "t")).unwrap();
    assert_eq!(OnExist::Ignore, config.on_exist);
    assert_eq!(
        Err(TableError::AlreadyExists),
        catalog.insert(catalog_table(2, 1, "t"))
    );
}

/// 验证 CREATE DATABASE IF NOT EXISTS 的配置：忽略已存在且预分配 ID。
#[test]
fn test_create_db_if_not_exists() {
    let config = create_table_config(Some(OnExist::Ignore), true);
    assert_eq!(OnExist::Ignore, config.on_exist);
    assert!(config.id_allocated);
}

/// 验证重复创建同名索引报 `DuplicateName`，以及 IF NOT EXISTS 配置取值。
#[test]
fn test_ddl_if_not_exists() {
    let mut table = index_table();
    add_index(&mut table, "idx", "a");
    assert_eq!(
        Err(IndexError::DuplicateName("idx".into())),
        build_index_info(
            &mut table,
            "idx",
            &[("a".into(), None)],
            IndexOptions::default()
        )
    );
    assert_eq!(
        OnExist::Ignore,
        create_table_config(Some(OnExist::Ignore), false).on_exist
    );
}

/// 验证删除不存在的索引报 `IndexNotFound`，未指定 IF NOT EXISTS 时默认报错。
#[test]
fn test_ddl_if_exists() {
    let mut table = index_table();
    assert_eq!(
        Err(IndexError::IndexNotFound("missing".into())),
        remove_index_info(&mut table, "missing")
    );
    assert_eq!(OnExist::Error, create_table_config(None, false).on_exist);
}

/// 验证 8 个线程并发提交 DDL 任务后，所有任务均登记成功且处于 Running 状态。
#[test]
fn test_parallel_ddl_before_run_ddl_job() {
    let ddl = Arc::new(Mutex::new(running_ddl()));
    let handles: Vec<_> = (1..=8)
        .map(|id| {
            let ddl = Arc::clone(&ddl);
            thread::spawn(move || {
                ddl.lock()
                    .unwrap()
                    .submit_job(job(id, "alter table t add column c int"))
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let ddl = ddl.lock().unwrap();
    assert_eq!(8, ddl.jobs.len());
    assert!(ddl.jobs.values().all(|job| job.state == JobState::Running));
}

/// 验证修改字符集/排序规则：首次生效返回 true，重复设置返回 false，
/// 不兼容的组合返回 `InvalidCharsetCollation`。
#[test]
fn test_parallel_alter_schema_charset_and_collate() {
    let mut table = catalog_table(1, 1, "t");
    assert!(alter_charset_and_collation(&mut table, "utf8mb4", "utf8mb4_general_ci").unwrap());
    assert!(!alter_charset_and_collation(&mut table, "utf8mb4", "utf8mb4_general_ci").unwrap());
    assert_eq!(
        Err(TableError::InvalidCharsetCollation),
        alter_charset_and_collation(&mut table, "utf8", "latin1_bin")
    );
}

/// 验证截断表（truncate：换新表 ID、回收旧数据）返回旧表 ID，随后仍可正常加列。
#[test]
fn test_parallel_truncate_table_and_add_column() {
    let mut catalog = TableCatalog::default();
    catalog.insert(catalog_table(1, 1, "t")).unwrap();
    assert_eq!(
        vec![1],
        catalog.truncate_table(1, "t", 2, Vec::new()).unwrap()
    );
    let mut columns = column_table(&["a"]);
    init_and_add_column_to_table(&mut columns, integer("b"));
    assert_eq!(2, columns.columns.len());
}

/// 验证截断分区表返回旧表 ID 与全部旧分区 ID，并可继续添加多列。
#[test]
fn test_parallel_truncate_table_and_add_columns() {
    let mut catalog = TableCatalog::default();
    let mut table = catalog_table(1, 1, "t");
    table.partition_ids = vec![11, 12];
    catalog.insert(table).unwrap();
    assert_eq!(
        vec![1, 11, 12],
        catalog.truncate_table(1, "t", 2, vec![21, 22]).unwrap()
    );
    let mut columns = column_table(&["a"]);
    for name in ["b", "c"] {
        init_and_add_column_to_table(&mut columns, integer(name));
    }
    assert_eq!(3, columns.columns.len());
}

/// 验证 Varchar -> DateTime 的列类型变更走完整 reorg 流程。
#[test]
fn test_write_reorg_for_column_type_change() {
    assert_reorg(ColumnKind::Varchar, ColumnKind::DateTime, false);
}

/// 验证创建非唯一表达式索引：索引指向的列是隐藏生成列。
#[test]
fn test_create_expression_index() {
    let mut table = index_table();
    let mut hidden = index_column("_V$_expression_index_0", ColumnType::Int, true);
    hidden.generated_dependencies.insert("a".into());
    table.columns.push(hidden);
    let index = add_index(&mut table, "expression_index", "_V$_expression_index_0");
    assert!(!index.unique);
    assert!(table.columns[index.columns[0].offset].hidden);
}

/// 验证创建唯一表达式索引：索引唯一且隐藏列被打上索引标记。
#[test]
fn test_create_unique_expression_index() {
    let mut table = index_table();
    let mut hidden = index_column("_V$_expression_index_0", ColumnType::Int, true);
    hidden.generated_dependencies.insert("a".into());
    table.columns.push(hidden);
    let index = build_index_info(
        &mut table,
        "expression_index",
        &[("_V$_expression_index_0".into(), None)],
        IndexOptions {
            unique: true,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    assert!(index.unique);
    assert_eq!(1, table.columns[index.columns[0].offset].index_flags);
}

/// 验证删除表达式索引后，其依赖的隐藏生成列被一并清理。
#[test]
fn test_drop_expression_index() {
    let mut table = index_table();
    let mut hidden = index_column("_V$_expression_index_0", ColumnType::Int, true);
    hidden.generated_dependencies.insert("a".into());
    table.columns.push(hidden);
    add_index(&mut table, "expression_index", "_V$_expression_index_0");
    let removed = remove_index_info(&mut table, "expression_index").unwrap();
    assert_eq!("expression_index", removed.name);
    assert!(table.columns.iter().all(|column| !column.hidden));
}

/// 验证原子交换两张表的名字（t1<->t2），双方 ID 互换成功。
#[test]
fn test_parallel_rename_table() {
    let mut catalog = TableCatalog::default();
    catalog.insert(catalog_table(1, 1, "t1")).unwrap();
    catalog.insert(catalog_table(2, 1, "t2")).unwrap();
    catalog
        .rename_tables(&[
            (1, "t1".into(), 1, "t2".into()),
            (1, "t2".into(), 1, "t1".into()),
        ])
        .unwrap();
    assert_eq!(2, catalog.get(1, "t1").unwrap().id);
    assert_eq!(1, catalog.get(1, "t2").unwrap().id);
}

/// 验证两个线程并发设置默认值后，结果为其中之一且两个默认值字段保持一致。
#[test]
fn test_concurrent_set_default_value() {
    let table = Arc::new(Mutex::new(column_table(&["a"])));
    let handles: Vec<_> = [1_i64, 2_i64]
        .into_iter()
        .map(|value| {
            let table = Arc::clone(&table);
            thread::spawn(move || {
                let mut table = table.lock().unwrap();
                set_default_for_modified_column(
                    &mut table.columns[0],
                    Some(DefaultValue::Integer(value)),
                );
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let table = table.lock().unwrap();
    assert_eq!(
        table.columns[0].default_value,
        table.columns[0].origin_default_value
    );
    assert!(matches!(
        table.columns[0].default_value,
        Some(DefaultValue::Integer(1 | 2))
    ));
}

/// 验证重命名索引以及重命名表达式索引引用的列名均反映到索引元数据。
#[test]
fn test_parallel_rename_index_metadata() {
    let mut table = index_table();
    add_index(&mut table, "old", "a");
    rename_index(&mut table, "old", "new").unwrap();
    rename_expression_index_columns(&mut table, "a", "renamed_a");
    assert_eq!("new", table.indices[0].name);
    assert_eq!("renamed_a", table.indices[0].columns[0].name);
}

/// 验证 DDL 任务的暂停/恢复权限：系统暂停的任务用户无权恢复，
/// 只能由系统恢复，任务完成后进入 Synced（已同步到所有节点）状态。
#[test]
fn test_parallel_job_pause_resume_preserves_owner() {
    let mut ddl = running_ddl();
    ddl.submit_job(job(1, "alter table t add index idx(a)"))
        .unwrap();
    assert!(ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::System)[0].is_ok());
    assert!(ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::User)[0].is_err());
    assert!(ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::System)[0].is_ok());
    ddl.finish_job(1).unwrap();
    assert_eq!(JobState::Synced, ddl.history[0].state);
}
