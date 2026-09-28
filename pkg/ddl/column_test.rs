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

// `column` 模块的单元测试：覆盖 DDL（数据定义语言，如 ADD/DROP/MODIFY COLUMN）
// 中列相关的核心操作，包括向表中添加列、定位列偏移量、删除列及其单列索引、
// 列数量上限校验、schema 状态（在线 DDL 状态机）可见性以及修改列时索引元数据的同步更新。
//
// 背景说明：在线 DDL 采用多阶段 schema 状态机（None -> DeleteOnly -> WriteOnly ->
// WriteReorganization -> Public），使得集群中不同节点即使 schema 版本相差一个状态
// 也能保持数据一致性；只有到达 Public 状态的列才对读写完全可见。

use crate::column::{
    ColumnError, ColumnInfo, ColumnKind, ColumnPosition, FieldType, IndexColumn, IndexInfo,
    SchemaState, TableInfo, check_add_column_too_many_columns, check_after_position_exists,
    init_and_add_column_to_table, locate_offset_to_move, remove_column_and_single_indices,
    update_index_column,
};

/// 构造一个整数类型的列元信息，作为测试的基础工具函数。
fn column(name: &str) -> ColumnInfo {
    ColumnInfo::new(name, FieldType::integer())
}

/// 构造一个已处于 Public（公开可见）状态的列，模拟 DDL 完成后的列。
fn public_column(name: &str) -> ColumnInfo {
    let mut column = column(name);
    column.state = SchemaState::Public;
    column
}

/// 构造一个只包含单个列的普通（非主键、非列存）索引元信息。
fn index(id: i64, name: &str, column_name: &str) -> IndexInfo {
    IndexInfo {
        id,
        name: name.into(),
        state: SchemaState::Public,
        columns: vec![IndexColumn {
            name: column_name.into(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    }
}

/// 基础流程测试：添加列、按位置（FIRST/AFTER）定位偏移量、
/// 引用不存在列时报错、删除列时级联删除仅含该列的索引。
#[test]
fn test_column_basic() {
    let mut table = TableInfo::new(1, "t");
    let a = init_and_add_column_to_table(&mut table, column("a"));
    let b = init_and_add_column_to_table(&mut table, column("b"));
    // 手动把所有列置为 Public，模拟 DDL 已完成、列对外可见的状态。
    for column in &mut table.columns {
        column.state = SchemaState::Public;
    }
    // 列 ID 从 1 开始递增分配。
    assert_eq!((1, 2), (a, b));
    // FIRST 表示移动到最前面，目标偏移量为 0。
    assert_eq!(
        Ok(0),
        locate_offset_to_move(1, &ColumnPosition::First, &table)
    );
    assert_eq!(
        Ok(1),
        locate_offset_to_move(1, &ColumnPosition::After("a".into()), &table)
    );
    // AFTER 引用的列不存在时应返回 ColumnNotFound 错误。
    assert_eq!(
        Err(ColumnError::ColumnNotFound("missing".into())),
        check_after_position_exists(&table, &ColumnPosition::After("missing".into()))
    );
    // 删除列 b 时，仅覆盖该列的索引 idx_b 也应一并删除，并返回被删索引的 ID。
    table.indices.push(index(10, "idx_b", "b"));
    assert_eq!(
        vec![10],
        remove_column_and_single_indices(&mut table, b).unwrap()
    );
    assert_eq!(
        vec!["a"],
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// 单列添加测试：验证新列的 ID、偏移量分配，以及新列初始进入
/// StateNone（在线 DDL 状态机的起始状态，对读写均不可见）。
#[test]
fn test_add_column() {
    let mut table = TableInfo::new(1, "t");
    let id = init_and_add_column_to_table(&mut table, public_column("a"));
    assert_eq!(1, id);
    assert_eq!(0, table.columns[0].offset);
    assert_eq!(
        SchemaState::None,
        table.columns[0].state,
        "new columns enter at StateNone"
    );
    // 列数未超过上限（512）时校验通过。
    assert!(check_add_column_too_many_columns(table.columns.len(), 512).is_ok());
}

/// 多列添加测试：验证列 ID 与偏移量按插入顺序递增分配，
/// 以及超过列数上限时返回 TooManyColumns 错误。
#[test]
fn test_add_columns() {
    let mut table = TableInfo::new(1, "t");
    for name in ["a", "b", "c"] {
        init_and_add_column_to_table(&mut table, column(name));
    }
    assert_eq!(
        vec![1, 2, 3],
        table
            .columns
            .iter()
            .map(|column| column.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        vec![0, 1, 2],
        table
            .columns
            .iter()
            .map(|column| column.offset)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        Err(ColumnError::TooManyColumns {
            count: 513,
            limit: 512
        }),
        check_add_column_too_many_columns(513, 512)
    );
}

/// 删除列测试：表中仅剩一列时禁止删除；删除普通列时级联删除其单列索引。
#[test]
fn test_drop_column_in_column_test() {
    let mut table = TableInfo::new(1, "t");
    let only = init_and_add_column_to_table(&mut table, public_column("a"));
    // 表至少要保留一列，删除唯一的列应报错。
    assert_eq!(
        Err(ColumnError::CannotDropOnlyColumn("a".into())),
        remove_column_and_single_indices(&mut table, only)
    );
    let b = init_and_add_column_to_table(&mut table, public_column("b"));
    table.indices.push(index(8, "idx_b", "b"));
    assert_eq!(
        vec![8],
        remove_column_and_single_indices(&mut table, b).unwrap()
    );
}

/// 连续删除多列的场景：删除中间的列后，剩余列的偏移量需要重新压缩为连续值。
#[test]
fn test_drop_columns_case() {
    let mut table = TableInfo::new(1, "t");
    let ids: Vec<_> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|name| init_and_add_column_to_table(&mut table, public_column(name)))
        .collect();
    // 删除中间的 b、c 两列。
    for id in [ids[1], ids[2]] {
        remove_column_and_single_indices(&mut table, id).unwrap();
    }
    assert_eq!(
        vec!["a", "d"],
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
    );
    // 剩余列 a、d 的偏移量被压缩回 0、1。
    assert_eq!(
        vec![0, 1],
        table
            .columns
            .iter()
            .map(|column| column.offset)
            .collect::<Vec<_>>()
    );
}

/// 验证在线 DDL 状态机中列对新写入的可见性：
/// 只有进入 WriteOnly 及之后的状态（WriteReorganization、Public），
/// 新增列才需要接收写入；None 与 DeleteOnly 阶段对写入不可见。
#[test]
fn test_write_data_write_only_mode() {
    let mut adding = column("b");
    let states = [
        SchemaState::None,
        SchemaState::DeleteOnly,
        SchemaState::WriteOnly,
        SchemaState::WriteReorganization,
        SchemaState::Public,
    ];
    // 按状态机顺序逐一切换状态，判断该状态下列是否对新写入可见。
    let visible_to_new_writes: Vec<_> = states
        .into_iter()
        .map(|state| {
            adding.state = state;
            state >= SchemaState::WriteOnly
        })
        .collect();
    assert_eq!(vec![false, false, true, true, true], visible_to_new_writes);
}

/// 修改列（MODIFY/CHANGE COLUMN）时同步更新索引列元数据：
/// 索引列的名称、偏移量需跟随新列，且当索引前缀长度超过新列长度时应被移除。
#[test]
fn test_modify_column_with_index() {
    // 模拟列被重命名为 renamed，类型改为 VARCHAR(4)，偏移量变为 2。
    let mut changing = public_column("renamed");
    changing.offset = 2;
    changing.field_type.kind = ColumnKind::Varchar;
    changing.field_type.flen = 4;
    let mut index_column = IndexColumn {
        name: "old".into(),
        offset: 0,
        length: Some(8),
        use_changing_type: true,
    };
    update_index_column(&mut index_column, &changing);
    // 索引列的名称与偏移量应同步为新列的值。
    assert_eq!("renamed", index_column.name);
    assert_eq!(2, index_column.offset);
    // 原前缀长度 8 超过新列长度 4，前缀索引长度应被清除。
    assert_eq!(
        None, index_column.length,
        "prefix longer than new column is removed"
    );

    // Go's types.IsTypePrefixable accepts every Blob/Text family type as well
    // as VARCHAR/STRING. A valid shorter prefix must therefore survive a
    // column change to BLOB.
    changing.field_type.kind = ColumnKind::Blob;
    changing.field_type.flen = 16;
    index_column.length = Some(8);
    update_index_column(&mut index_column, &changing);
    assert_eq!(
        Some(8),
        index_column.length,
        "blob columns retain a valid prefix like Go UpdateIndexCol"
    );
}
