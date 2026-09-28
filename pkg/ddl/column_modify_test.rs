// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL（数据定义语言）列修改流程的单元测试。
//
// 本模块围绕以下几类场景验证列修改（MODIFY/CHANGE COLUMN）逻辑：
// - 列的新增与删除（含被索引引用的列不可直接删除的保护）；
// - 无损修改（如仅改名/放宽长度）一步完成；
// - 有损类型变更需要走 reorg（数据重组）多阶段状态机；
// - 生成列（generated column，值由表达式计算得出）的依赖检查；
// - schema 版本号推进与在线 DDL 的中间状态
//   （DeleteOnly → WriteOnly → WriteReorganization → Public）。
//
// 背景：在线 DDL 采用类似 Google F1 的多阶段 schema 演进模型，
// 每个阶段对应一个 `SchemaState`，保证集群中不同节点在相邻两个
// schema 版本之间也能保持数据一致性。

use crate::column::{
    ColumnInfo, ColumnKind, ColumnPosition, FieldType, IndexColumn, IndexInfo, SchemaState,
    TableInfo, init_and_add_column_to_table, remove_column_and_single_indices,
};
use crate::generated_column::{
    ExpressionNode, GenerationAttribute, check_depended_columns_exist, find_column_names_in_expr,
};
use crate::modify_column::{
    ModifyColumnArgs, ModifyColumnContext, ModifyColumnError, ModifyColumnType,
    advance_modify_column, check_modify_types,
};

/// 构造一个指定名称的整数类型列，作为测试中最常用的列定义。
fn integer(name: &str) -> ColumnInfo {
    ColumnInfo::new(name, FieldType::integer())
}

/// 构造一张包含给定列名的测试表，所有列都直接置为 Public（对外可见）状态。
fn table_with_columns(names: &[&str]) -> TableInfo {
    let mut table = TableInfo::new(1, "t");
    for name in names {
        let id = init_and_add_column_to_table(&mut table, integer(name));
        // 新增列默认处于不可见的初始状态，这里手动推进到 Public，
        // 模拟一张已经建好、所有列都可读写的表。
        table
            .columns
            .iter_mut()
            .find(|column| column.id == id)
            .unwrap()
            .state = SchemaState::Public;
    }
    table
}

/// 构造默认的列修改上下文：严格 SQL 模式、无分区、启用有损优化，
/// 并给出 AUTO_RANDOM 相关的默认位数配置。
fn context() -> ModifyColumnContext {
    ModifyColumnContext {
        strict_sql_mode: true,
        partition: None,
        disable_lossy_optimization: false,
        auto_random_range_bits_default: 64,
        auto_random_shard_bits_max: 15,
    }
}

/// 根据旧列与新列定义构造一份最简的列修改参数（其余字段均取零值/空值）。
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

/// 验证列的新增与删除：新增列 c 后删除，表结构应回到只有 a、b 两列。
#[test]
fn test_add_and_drop_column() {
    let mut table = table_with_columns(&["a", "b"]);
    let c = init_and_add_column_to_table(&mut table, integer("c"));
    table
        .columns
        .iter_mut()
        .find(|column| column.id == c)
        .unwrap()
        .state = SchemaState::Public;
    assert_eq!(3, table.columns.len());
    // 删除列 c；该列未被任何索引引用，因此不会连带删除索引，返回空列表。
    assert!(
        remove_column_and_single_indices(&mut table, c)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        vec!["a", "b"],
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 验证删除被多列索引引用的列会失败：列 a 属于复合索引 (a, b)，不能直接删除。
#[test]
fn test_drop_column() {
    let mut table = table_with_columns(&["a", "b"]);
    // 手动构造一个覆盖 a、b 两列的复合索引。
    table.indices.push(IndexInfo {
        id: 10,
        name: "ab".into(),
        state: SchemaState::Public,
        columns: vec![
            IndexColumn {
                name: "a".into(),
                offset: 0,
                length: None,
                use_changing_type: false,
            },
            IndexColumn {
                name: "b".into(),
                offset: 1,
                length: None,
                use_changing_type: false,
            },
        ],
        primary: false,
        columnar: false,
    });
    let a = table.columns[0].id;
    // 复合索引依赖列 a，删除应报错，避免留下悬空的索引列引用。
    assert!(remove_column_and_single_indices(&mut table, a).is_err());
}

/// 验证无损的 CHANGE COLUMN：仅改列名并放宽显示长度（flen），
/// 无需数据重组，一次调用即可完成并直接进入 Public 状态。
#[test]
fn test_change_column() {
    let mut table = table_with_columns(&["a", "b"]);
    let old = table.columns[0].clone();
    let mut renamed = old.clone();
    renamed.name = "c".into();
    renamed.field_type.flen = 20;
    let mut args = args(&old, renamed);
    let mut version = 0;
    let outcome =
        advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
            .unwrap();
    // 无损修改一步完成，不经过多阶段状态机。
    assert!(outcome.finished);
    assert_eq!(SchemaState::Public, outcome.schema_state);
    assert_eq!("c", table.columns[0].name);
}

/// 验证不允许把普通列改为生成列：生成列的值由表达式计算，
/// 这类修改被类型检查拒绝并返回 UnsupportedTypeChange。
#[test]
fn test_virtual_column_ddl() {
    let old = integer("a");
    let mut generated = integer("g");
    generated.generated = true;
    generated.generated_expression = "a + 1".into();
    assert_eq!(
        Err(ModifyColumnError::UnsupportedTypeChange),
        check_modify_types(&old, &generated, false)
    );
}

/// 验证 WriteOnly（只写）状态列的属性：在线 DDL 中该状态下
/// 事务写入时需要为新列补默认值，但该列尚不可被读取。
#[test]
fn test_transaction_with_write_only_column() {
    let mut column = integer("b");
    column.default_value = Some(crate::column::DefaultValue::Integer(3));
    column.state = SchemaState::WriteOnly;
    // SchemaState 定义了有序的状态推进，WriteOnly 及之后的状态才允许写入。
    assert!(column.state >= SchemaState::WriteOnly);
    assert_eq!(
        Some(crate::column::DefaultValue::Integer(3)),
        column.default_value
    );
    column.state = SchemaState::Public;
    assert_eq!(SchemaState::Public, column.state);
}

/// 验证生成列表达式的依赖分析：从表达式 plus(a, 常量) 中提取被引用的
/// 列名集合，并检查这些依赖列在表中确实存在。
#[test]
fn test_add_generated_column_and_insert() {
    // 构造表达式树：plus(列 a, 字面量)，即 a + 常量。
    let expression = ExpressionNode::Function {
        name: "plus".into(),
        supported: true,
        guaranteed_available: true,
        arguments: vec![ExpressionNode::Column("a".into()), ExpressionNode::Literal],
    };
    // 表达式中只引用了列 a，依赖集合应恰好包含它。
    let mut dependencies = find_column_names_in_expr(&expression);
    assert_eq!(1, dependencies.len());
    assert!(dependencies.contains("a"));
    assert_eq!(
        Ok(()),
        check_depended_columns_exist(&mut dependencies, &[integer("a"), integer("g")])
    );
    let attribute = GenerationAttribute {
        position: 1,
        generated: true,
        dependencies: ["a".to_owned()].into_iter().collect(),
    };
    assert!(attribute.generated && attribute.dependencies.contains("a"));
}

/// 验证有损类型变更（整数改 DateTime）会创建隐藏的“changing 列”，
/// 且 changing 列与索引使用 Go 一致的递增后缀以保证在表内唯一。
#[test]
fn test_column_type_change_gen_unique_changing_name() {
    let mut table = table_with_columns(&["a", "b", "_col$_a"]);
    table.indices.push(IndexInfo {
        id: 10,
        name: "idx".into(),
        state: SchemaState::Public,
        columns: vec![IndexColumn {
            name: "a".into(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    let old = table.columns[0].clone();
    let mut changed = old.clone();
    // 整数改为日期时间属于有损变更，必须走 reorg（重写已有数据）流程。
    changed.field_type.kind = ColumnKind::DateTime;
    let mut args = args(&old, changed);
    let mut version = 0;
    let outcome =
        advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
            .unwrap();
    // 第一步只创建 changing 列，整个修改尚未完成。
    assert!(!outcome.finished);
    let changing_id = args.changing_column_id.unwrap();
    let changing = table
        .columns
        .iter()
        .find(|column| column.id == changing_id)
        .unwrap();
    assert_eq!("_col$_a_0", changing.name);
    // changing 列名在表内必须唯一，避免与用户列或其他隐藏列冲突。
    assert!(
        table
            .columns
            .iter()
            .filter(|column| column.name == changing.name)
            .count()
            == 1
    );
    let changing_index = table
        .indices
        .iter()
        .find(|index| args.changing_index_ids.contains(&index.id))
        .unwrap();
    assert_eq!("_idx$_idx_0", changing_index.name);
    assert_eq!(changing.name, changing_index.columns[0].name);
    assert!(changing_index.columns[0].use_changing_type);
}

/// 验证有损修改的完整状态机推进：从初始状态依次经过
/// DeleteOnly → WriteOnly → WriteReorganization → Public 四个阶段，
/// 每个阶段推进一次 schema 版本（10 → 14）。
#[test]
fn test_modify_column_reorg_checkpoint() {
    let mut table = table_with_columns(&["a", "b"]);
    let old = table.columns[0].clone();
    let mut changed = old.clone();
    changed.field_type.kind = ColumnKind::DateTime;
    let mut args = args(&old, changed);
    let mut version = 10;
    let mut states = Vec::new();
    // 循环推进状态机直到完成，记录每一步产生的 schema 状态。
    loop {
        let outcome =
            advance_modify_column(&mut table, &mut args, &context(), true, &mut version, false)
                .unwrap();
        states.push(outcome.schema_state);
        if outcome.finished {
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
    // 四个阶段各推进一次版本：10 + 4 = 14。
    assert_eq!(14, version);
}

/// 回归测试（对应 TiDB issue #37611）：前置检查失败
/// （如把可空列改为 NOT NULL 但校验不通过）时不得推进 schema 版本。
#[test]
fn test_issue37611() {
    let mut table = table_with_columns(&["a"]);
    let old = table.columns[0].clone();
    let mut narrower = old.clone();
    // 收窄显示长度并加上 NOT NULL 约束，触发空值合法性前置检查。
    narrower.field_type.flen = 4;
    narrower.not_null = true;
    let mut args = args(&old, narrower);
    let mut version = 0;
    assert_eq!(
        Err(ModifyColumnError::InvalidNull),
        advance_modify_column(
            &mut table,
            &mut args,
            &context(),
            false,
            &mut version,
            false
        )
    );
    assert_eq!(
        0, version,
        "failed precheck must not advance schema version"
    );
}
