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

// 外键（Foreign Key）DDL 测试模块。
//
// 外键是关系数据库中的引用完整性约束：子表（child）的某些列必须引用
// 父表（parent）中已存在的键值。本文件测试 DDL 执行器对外键的
// 增加（add foreign key）与删除（drop foreign key）生命周期管理。
//

// 下面的活动测试使用内存版 DDL 执行器，只覆盖当前已经接通的最小外键行为。
use std::time::Duration;

// 引入 DDL 执行器相关类型：
// - Executor：DDL 语句的执行入口；
// - MemoryJobBackend：内存版 DDL job（任务）存储后端，测试专用；
// - SessionContext：会话上下文，保存会话级变量与状态；
// - TableInfo / ColumnInfo / ForeignKeyInfo：表、列、外键的元数据描述；
// - Ident：带 schema 限定的对象标识符（schema.table）。
use crate::executor::{
    ColumnInfo, Executor, ExecutorError, ForeignKeyInfo, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};

/// 验证外键的完整生命周期：
/// 1. 添加外键成功（父表存在、列匹配）；
/// 2. 重复添加同名外键报 `ForeignKeyExists`；
/// 3. 删除外键时名称大小写不敏感（用 "FK_PARENT" 删除 "fk_parent"）；
/// 4. 重复删除已不存在的外键报 `ForeignKeyNotFound`。
#[test]
fn foreign_key_lifecycle_checks_referenced_table_and_duplicate_names() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 准备环境：创建 test 库以及 parent/child 两张表。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("parent", vec![ColumnInfo::integer("id")]),
        OnExist::Error,
    )
    .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("child", vec![ColumnInfo::integer("parent_id")]),
        OnExist::Error,
    )
    .unwrap();
    // 先缓存 child 表标识，后续 add/drop foreign key 都围绕这张子表执行。
    let child = Ident::new("test", "child");
    // 外键定义：child.parent_id 引用 parent.id。
    let fk = ForeignKeyInfo::new(
        "fk_parent",
        vec!["parent_id".into()],
        Ident::new("test", "parent"),
        vec!["id".into()],
    );
    // 第一次添加应成功。
    ddl.add_foreign_key(&mut session, &child, fk.clone())
        .unwrap();
    // 再次提交同名外键定义，验证执行器会在元数据层拒绝重复名字。
    // 同名外键重复添加应报 ForeignKeyExists。
    assert!(matches!(
        ddl.add_foreign_key(&mut session, &child, fk),
        Err(ExecutorError::ForeignKeyExists(_))
    ));
    // 外键名比较大小写不敏感：大写名称也能删除小写定义的外键。
    ddl.drop_foreign_key(&mut session, &child, "FK_PARENT")
        .unwrap();
    // 已删除的外键再次删除应报 ForeignKeyNotFound。
    assert!(matches!(
        ddl.drop_foreign_key(&mut session, &child, "fk_parent"),
        Err(ExecutorError::ForeignKeyNotFound(_))
    ));
}

/// 验证被引用的父表不存在时，添加外键应报 `TableNotFound`。
#[test]
fn foreign_key_rejects_missing_parent() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 只创建 child 表，故意不创建被引用的 parent 表。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("child", vec![ColumnInfo::integer("id")]),
        OnExist::Error,
    )
    .unwrap();
    // 构造一条指向缺失父表的外键定义，用来验证引用完整性前置检查。
    let fk = ForeignKeyInfo::new(
        "fk",
        vec!["id".into()],
        Ident::new("test", "parent"),
        vec!["id".into()],
    );
    // 父表 test.parent 不存在，添加外键应失败并返回 TableNotFound。
    assert!(matches!(
        ddl.add_foreign_key(&mut session, &Ident::new("test", "child"), fk),
        Err(ExecutorError::TableNotFound(_))
    ));
}

#[test]
fn foreign_key_definition_compares_charset_and_collation_for_every_type() {
    use crate::column::{
        ColumnInfo as DdlColumnInfo, FieldType, IndexColumn, IndexInfo, SchemaState,
        TableInfo as DdlTableInfo,
    };
    use crate::foreign_key::{
        ForeignKeyError, ForeignKeyInfo as DdlForeignKeyInfo, ForeignKeyTable, ReferentialAction,
        check_foreign_key_definition,
    };

    let make_table = |id, name: &str, charset: &str| {
        let mut field_type = FieldType::integer();
        field_type.charset = charset.to_owned();
        field_type.collation = charset.to_owned();
        let mut column = DdlColumnInfo::new("id", field_type);
        column.id = 1;
        column.state = SchemaState::Public;
        let mut table = DdlTableInfo::new(id, name);
        table.columns.push(column);
        table.indices.push(IndexInfo {
            id: 1,
            name: "idx_id".to_owned(),
            state: SchemaState::Public,
            columns: vec![IndexColumn {
                name: "id".to_owned(),
                offset: 0,
                length: None,
                use_changing_type: false,
            }],
            primary: false,
            columnar: false,
        });
        ForeignKeyTable {
            schema_name: "test".to_owned(),
            temporary: false,
            partitioned: false,
            ttl_enabled: false,
            primary_key_is_handle: false,
            table,
            max_foreign_key_id: 0,
            foreign_keys: Vec::new(),
        }
    };
    let parent = make_table(1, "parent", "binary");
    let child = make_table(2, "child", "utf8mb4");
    let foreign_key = DdlForeignKeyInfo {
        id: 0,
        name: "fk".to_owned(),
        columns: vec!["id".to_owned()],
        referenced_schema: "test".to_owned(),
        referenced_table: "parent".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::Restrict,
        on_update: ReferentialAction::Restrict,
        version: 1,
        state: SchemaState::None,
    };

    assert_eq!(
        check_foreign_key_definition(&parent, &child, &foreign_key),
        Err(ForeignKeyError::IncompatibleColumns(
            "id".to_owned(),
            "id".to_owned()
        ))
    );
}

#[test]
fn dropping_redundant_index_accepts_primary_key_handle_like_go() {
    use crate::column::{
        ColumnInfo as DdlColumnInfo, FieldType, IndexColumn, IndexInfo, SchemaState,
        TableInfo as DdlTableInfo,
    };
    use crate::foreign_key::{
        ForeignKeyCatalog, ForeignKeyInfo as DdlForeignKeyInfo, ForeignKeyTable, ReferentialAction,
        check_index_needed_in_foreign_key,
    };

    let mut column = DdlColumnInfo::new("id", FieldType::integer());
    column.id = 1;
    column.state = SchemaState::Public;
    let mut metadata = DdlTableInfo::new(1, "parent");
    metadata.columns.push(column);
    metadata.indices.push(IndexInfo {
        id: 10,
        name: "idx_id".to_owned(),
        state: SchemaState::Public,
        columns: vec![IndexColumn {
            name: "id".to_owned(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    let mut parent = ForeignKeyTable {
        schema_name: "test".to_owned(),
        temporary: false,
        partitioned: false,
        ttl_enabled: false,
        primary_key_is_handle: true,
        table: metadata,
        max_foreign_key_id: 0,
        foreign_keys: Vec::new(),
    };
    parent.foreign_keys.push(DdlForeignKeyInfo {
        id: 1,
        name: "fk".to_owned(),
        columns: vec!["id".to_owned()],
        referenced_schema: "test".to_owned(),
        referenced_table: "parent".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::Restrict,
        on_update: ReferentialAction::Restrict,
        version: 1,
        state: SchemaState::Public,
    });
    let mut catalog = ForeignKeyCatalog {
        enabled: true,
        ..Default::default()
    };
    catalog.add_table(parent);

    check_index_needed_in_foreign_key(&catalog, "test", "parent", 10).unwrap();
}
