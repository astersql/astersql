// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 外键（Foreign Key）DDL 单元测试。
//
// 不依赖真实 SQL 执行器，直接构造 `ForeignKeyCatalog` / `ForeignKeyTable`
// 元数据，覆盖：
// - 创建外键的 SchemaState 状态机（None → WriteOnly → WriteReorganization → Public）；
// - 定义校验（父表索引、列类型兼容、临时表/分区/TTL 限制）；
// - 引用保护（删父表/索引/列、子表列变更）；
// - 引用动作（ON DELETE/UPDATE：Cascade/SetNull/Restrict/NoAction）；
// - 现有行冲突回滚、检查 SQL 生成、大小写不敏感查找。
//
// SchemaState：DDL 对象对会话可见性的渐进状态；Public 表示对所有会话可见。

use astersql_ddl::column::{
    ColumnInfo, ColumnKind, FieldType, IndexColumn, IndexInfo, SchemaState, TableInfo,
};
use astersql_ddl::foreign_key::{
    ForeignKeyCatalog, ForeignKeyError, ForeignKeyInfo, ForeignKeyTable, ReferentialAction,
    advance_create_foreign_key, build_foreign_key_check_sql, check_add_foreign_key_valid,
    check_drop_column_with_foreign_key, check_foreign_key_definition,
    check_index_needed_in_foreign_key, check_modify_column_with_foreign_key,
    check_table_has_foreign_key_referred, drop_foreign_key,
    is_acceptable_foreign_key_column_change,
};
use astersql_testkit::{TestKit, mockstore::CreateMockStoreAndDomain};

/// 构造测试用列：整型默认，VARCHAR/STRING 附加 utf8mb4 字符集与长度。
fn column(id: i64, name: &str, kind: ColumnKind) -> ColumnInfo {
    let mut field_type = FieldType::integer();
    field_type.kind = kind;
    if matches!(kind, ColumnKind::Varchar | ColumnKind::String) {
        field_type.charset = "utf8mb4".to_owned();
        field_type.collation = "utf8mb4_bin".to_owned();
        field_type.flen = 32;
    }
    let mut column = ColumnInfo::new(name, field_type);
    column.id = id;
    column.state = SchemaState::Public;
    column
}

/// 构造非主键、非 columnar 的二级索引元数据。
fn index(id: i64, name: &str, columns: &[&str]) -> IndexInfo {
    IndexInfo {
        id,
        name: name.to_owned(),
        state: SchemaState::Public,
        columns: columns
            .iter()
            .enumerate()
            .map(|(offset, name)| IndexColumn {
                name: (*name).to_owned(),
                offset,
                length: None,
                use_changing_type: false,
            })
            .collect(),
        primary: false,
        columnar: false,
    }
}

/// 组装带 schema 名的 `ForeignKeyTable`（外键相关的表视图）。
fn table(
    id: i64,
    schema: &str,
    name: &str,
    columns: Vec<ColumnInfo>,
    indices: Vec<IndexInfo>,
) -> ForeignKeyTable {
    let mut metadata = TableInfo::new(id, name);
    metadata.columns = columns;
    metadata.indices = indices;
    ForeignKeyTable {
        schema_name: schema.to_owned(),
        temporary: false,
        partitioned: false,
        ttl_enabled: false,
        primary_key_is_handle: false,
        table: metadata,
        max_foreign_key_id: 0,
        foreign_keys: Vec::new(),
    }
}

/// 默认外键：`child.parent_id` → `test.parent.id`，ON DELETE CASCADE / ON UPDATE SET NULL。
fn foreign_key(name: &str) -> ForeignKeyInfo {
    ForeignKeyInfo {
        id: 0,
        name: name.to_owned(),
        columns: vec!["parent_id".to_owned()],
        referenced_schema: "test".to_owned(),
        referenced_table: "parent".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::Cascade,
        on_update: ReferentialAction::SetNull,
        version: 1,
        state: SchemaState::None,
    }
}

/// 构造启用外键检查的标准 parent/child 目录（双方均有所需索引）。
fn catalog() -> ForeignKeyCatalog {
    let mut catalog = ForeignKeyCatalog {
        enabled: true,
        ..Default::default()
    };
    catalog.add_table(table(
        1,
        "test",
        "parent",
        vec![column(1, "id", ColumnKind::Integer)],
        vec![index(1, "parent_id", &["id"])],
    ));
    catalog.add_table(table(
        2,
        "test",
        "child",
        vec![
            column(1, "id", ColumnKind::Integer),
            column(2, "parent_id", ColumnKind::Integer),
        ],
        vec![index(2, "child_parent_id", &["parent_id"])],
    ));
    catalog
}

/// CREATE TABLE 必须与 Go 一样在外键检查开启时拒绝不存在的父表。
#[test]
fn create_table_rejects_missing_foreign_key_parent() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create database fk_audit", Vec::new());
    testkit.MustExec("use fk_audit", Vec::new());
    testkit.MustGetErrMsg(
        "create table child (id int primary key, parent_id int, \
         foreign key fk_parent(parent_id) references missing_parent(id))",
        "[schema:1824]Failed to open the referenced table 'missing_parent'",
    );
}

/// CREATE TABLE 父表解析保留 Go 的自引用与关闭检查分支。
#[test]
fn create_table_foreign_key_parent_validation_honors_go_exceptions() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create database fk_audit_exceptions", Vec::new());
    testkit.MustExec("use fk_audit_exceptions", Vec::new());
    testkit.MustExec(
        "create table self_parent (id int primary key, parent_id int, \
         foreign key fk_parent(parent_id) references self_parent(id))",
        Vec::new(),
    );
    testkit.MustExec("set @@foreign_key_checks=0", Vec::new());
    testkit.MustExec(
        "create table deferred_child (id int primary key, parent_id int, \
         foreign key fk_parent(parent_id) references deferred_parent(id))",
        Vec::new(),
    );
}

/// 创建外键沿 SchemaState 推进，并校验最终元数据与重复名报错。
#[test]
fn foreign_key_metadata_actions_and_schema_states_are_preserved() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk_child_parent");
    let mut schema_version = 10;

    // 校验通过后进入 WriteOnly（仅写路径可见）。
    check_add_foreign_key_valid(&catalog, "test", "child", &foreign_key, true).unwrap();
    let write_only = advance_create_foreign_key(
        &mut catalog,
        "test",
        "child",
        &mut foreign_key,
        true,
        true,
        &mut schema_version,
        false,
    )
    .unwrap();
    assert_eq!(write_only.schema_state, SchemaState::WriteOnly);
    assert_eq!(foreign_key.id, 1);
    assert_eq!(schema_version, 11);

    // WriteReorganization：后台检查/回填阶段，尚未 finished。
    let reorganization = advance_create_foreign_key(
        &mut catalog,
        "test",
        "child",
        &mut foreign_key,
        true,
        true,
        &mut schema_version,
        false,
    )
    .unwrap();
    assert_eq!(
        reorganization.schema_state,
        SchemaState::WriteReorganization
    );
    assert!(!reorganization.finished);

    // Public：对外可见，作业完成。
    let public = advance_create_foreign_key(
        &mut catalog,
        "test",
        "child",
        &mut foreign_key,
        true,
        true,
        &mut schema_version,
        false,
    )
    .unwrap();
    assert_eq!(public.schema_state, SchemaState::Public);
    assert!(public.finished);
    assert_eq!(schema_version, 13);

    // 目录查找大小写不敏感，动作与引用列应原样保留。
    let stored = &catalog.table("TEST", "CHILD").unwrap().foreign_keys[0];
    assert_eq!(stored.name, "fk_child_parent");
    assert_eq!(stored.columns, ["parent_id"]);
    assert_eq!(stored.referenced_schema, "test");
    assert_eq!(stored.referenced_table, "parent");
    assert_eq!(stored.referenced_columns, ["id"]);
    assert_eq!(stored.on_delete, ReferentialAction::Cascade);
    assert_eq!(stored.on_update, ReferentialAction::SetNull);
    assert_eq!(stored.version, 1);
    assert_eq!(stored.state, SchemaState::Public);

    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "child", stored, true),
        Err(ForeignKeyError::DuplicateName("fk_child_parent".to_owned()))
    );
}

/// 定义校验：缺失父表、缺少父索引、列类型不兼容、临时表均应报错。
#[test]
fn foreign_key_definition_reports_parent_index_and_column_errors() {
    let mut catalog = catalog();
    let foreign_key = foreign_key("fk");

    // 严格模式下找不到父表；非严格（check=false）可跳过。
    let missing_parent = ForeignKeyInfo {
        referenced_table: "missing".to_owned(),
        ..foreign_key.clone()
    };
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "child", &missing_parent, true),
        Err(ForeignKeyError::CannotOpenParent("missing".to_owned()))
    );
    check_add_foreign_key_valid(&catalog, "test", "child", &missing_parent, false).unwrap();

    // 父表被引用列必须有可用索引。
    catalog
        .table_mut("test", "parent")
        .unwrap()
        .table
        .indices
        .clear();
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "child", &foreign_key, true),
        Err(ForeignKeyError::MissingIndex("fk".to_owned()))
    );

    // 父子列类型不一致（INT vs VARCHAR）→ IncompatibleColumns。
    let parent = catalog.table_mut("test", "parent").unwrap();
    parent.table.indices.push(index(1, "parent_id", &["id"]));
    parent.table.columns[0] = column(1, "id", ColumnKind::Varchar);
    assert_eq!(
        check_foreign_key_definition(
            catalog.table("test", "parent").unwrap(),
            catalog.table("test", "child").unwrap(),
            &foreign_key,
        ),
        Err(ForeignKeyError::IncompatibleColumns(
            "parent_id".to_owned(),
            "id".to_owned()
        ))
    );

    catalog.table_mut("test", "parent").unwrap().temporary = true;
    assert_eq!(
        check_foreign_key_definition(
            catalog.table("test", "parent").unwrap(),
            catalog.table("test", "child").unwrap(),
            &foreign_key,
        ),
        Err(ForeignKeyError::TemporaryTable)
    );
}

/// 引用保护：父表被引用时禁止删除；唯一支撑索引与外键列不可删。
#[test]
fn foreign_key_guards_protect_parent_table_index_and_columns() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk");
    foreign_key.state = SchemaState::Public;
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key);

    // 删除父表时若仍有子表引用则失败；exclude 列表可放行同事务内一并删除的子表。
    assert_eq!(
        check_table_has_foreign_key_referred(&catalog, "test", "parent", &[], true),
        Err(ForeignKeyError::ParentIsReferenced {
            table: "parent".to_owned(),
            foreign_key: "fk".to_owned(),
            child_table: "child".to_owned(),
        })
    );
    check_table_has_foreign_key_referred(
        &catalog,
        "test",
        "parent",
        &[("test".to_owned(), "child".to_owned())],
        true,
    )
    .unwrap();
    check_table_has_foreign_key_referred(&catalog, "test", "parent", &[], false).unwrap();

    // 父表上被外键依赖的索引不可删；存在冗余等价索引后可删原索引。
    assert_eq!(
        check_index_needed_in_foreign_key(&catalog, "test", "parent", 1),
        Err(ForeignKeyError::IndexNeeded("parent_id".to_owned()))
    );
    catalog
        .table_mut("test", "parent")
        .unwrap()
        .table
        .indices
        .push(index(3, "parent_id_redundant", &["id"]));
    check_index_needed_in_foreign_key(&catalog, "test", "parent", 1).unwrap();

    assert_eq!(
        check_drop_column_with_foreign_key(&catalog, "test", "child", "PARENT_ID"),
        Err(ForeignKeyError::ColumnNeeded(
            "PARENT_ID".to_owned(),
            "fk".to_owned()
        ))
    );
    assert_eq!(
        check_drop_column_with_foreign_key(&catalog, "test", "parent", "ID"),
        Err(ForeignKeyError::ColumnNeeded(
            "ID".to_owned(),
            "fk".to_owned()
        ))
    );
}

/// 现有行违反外键时，创建状态机应回滚元数据并清理半成品外键。
#[test]
fn foreign_key_existing_row_failure_rolls_back_metadata() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk");
    let mut schema_version = 0;

    // 先推进到 WriteOnly。
    advance_create_foreign_key(
        &mut catalog,
        "test",
        "child",
        &mut foreign_key,
        true,
        true,
        &mut schema_version,
        false,
    )
    .unwrap();
    // check_existing_rows=false 模拟发现违规行。
    assert_eq!(
        advance_create_foreign_key(
            &mut catalog,
            "test",
            "child",
            &mut foreign_key,
            true,
            false,
            &mut schema_version,
            false,
        ),
        Err(ForeignKeyError::ExistingRowsViolate("fk".to_owned()))
    );
    assert_eq!(foreign_key.state, SchemaState::WriteOnly);
    assert_eq!(schema_version, 1);

    // rollback=true：回到 None 并从 catalog 移除外键。
    let rolled_back = advance_create_foreign_key(
        &mut catalog,
        "test",
        "child",
        &mut foreign_key,
        true,
        false,
        &mut schema_version,
        true,
    )
    .unwrap();
    assert_eq!(rolled_back.schema_state, SchemaState::None);
    assert!(rolled_back.finished);
    assert!(rolled_back.rollback_done);
    assert_eq!(schema_version, 2);
    assert!(
        catalog
            .table("test", "child")
            .unwrap()
            .foreign_keys
            .is_empty()
    );
}

/// DROP FOREIGN KEY 名称大小写不敏感；重复删除报 ForeignKeyNotFound。
#[test]
fn drop_foreign_key_is_case_insensitive_and_reports_missing_names() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk_child_parent");
    foreign_key.state = SchemaState::Public;
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key);
    let mut schema_version = 7;

    let outcome = drop_foreign_key(
        &mut catalog,
        "test",
        "child",
        "FK_CHILD_PARENT",
        &mut schema_version,
        false,
    )
    .unwrap();
    assert_eq!(outcome.schema_state, SchemaState::None);
    assert!(outcome.finished);
    assert!(!outcome.rollback_done);
    assert_eq!(schema_version, 8);
    assert_eq!(
        drop_foreign_key(
            &mut catalog,
            "test",
            "child",
            "fk_child_parent",
            &mut schema_version,
            false,
        ),
        Err(ForeignKeyError::ForeignKeyNotFound(
            "fk_child_parent".to_owned()
        ))
    );
}

/// 定义规则矩阵：临时表、TTL 父表、分区表、列数不匹配、自引用同列等。
#[test]
fn foreign_key_definition_validation_matrix_matches_catalog_rules() {
    let base = catalog();
    let foreign_key = foreign_key("fk");

    // TTL（Time To Live）：表数据按时间自动过期清理，不可作为外键父表。
    for (mut parent, mut child, expected) in [
        (
            base.table("test", "parent").unwrap().clone(),
            base.table("test", "child").unwrap().clone(),
            ForeignKeyError::TemporaryTable,
        ),
        (
            base.table("test", "parent").unwrap().clone(),
            base.table("test", "child").unwrap().clone(),
            ForeignKeyError::TtlParent,
        ),
        (
            base.table("test", "parent").unwrap().clone(),
            base.table("test", "child").unwrap().clone(),
            ForeignKeyError::PartitionedTable,
        ),
    ] {
        match expected {
            ForeignKeyError::TemporaryTable => parent.temporary = true,
            ForeignKeyError::TtlParent => parent.ttl_enabled = true,
            ForeignKeyError::PartitionedTable => child.partitioned = true,
            _ => unreachable!(),
        }
        assert_eq!(
            check_foreign_key_definition(&parent, &child, &foreign_key),
            Err(expected)
        );
    }

    let parent = base.table("test", "parent").unwrap();
    let child = base.table("test", "child").unwrap();
    // 空列或父子列数不一致 → CannotAddForeignKey。
    for invalid in [
        ForeignKeyInfo {
            columns: Vec::new(),
            referenced_columns: Vec::new(),
            ..foreign_key.clone()
        },
        ForeignKeyInfo {
            columns: vec!["parent_id".to_owned(), "id".to_owned()],
            referenced_columns: vec!["id".to_owned()],
            ..foreign_key.clone()
        },
    ] {
        assert_eq!(
            check_foreign_key_definition(parent, child, &invalid),
            Err(ForeignKeyError::CannotAddForeignKey)
        );
    }

    let mut temporary_child = child.clone();
    temporary_child.temporary = true;
    assert_eq!(
        check_foreign_key_definition(parent, &temporary_child, &foreign_key),
        Err(ForeignKeyError::TemporaryTable)
    );
    let mut partitioned_parent = parent.clone();
    partitioned_parent.partitioned = true;
    assert_eq!(
        check_foreign_key_definition(&partitioned_parent, child, &foreign_key),
        Err(ForeignKeyError::PartitionedTable)
    );

    let missing_child = ForeignKeyInfo {
        columns: vec!["missing_child".to_owned()],
        ..foreign_key.clone()
    };
    assert_eq!(
        check_foreign_key_definition(parent, child, &missing_child),
        Err(ForeignKeyError::ColumnNotFound("missing_child".to_owned()))
    );
    let missing_parent = ForeignKeyInfo {
        referenced_columns: vec!["missing_parent".to_owned()],
        ..foreign_key.clone()
    };
    assert_eq!(
        check_foreign_key_definition(parent, child, &missing_parent),
        Err(ForeignKeyError::ColumnNotFound("missing_parent".to_owned()))
    );

    // 自引用但映射到同一列集合时拒绝（需列集合不同）。
    let mut self_table = child.clone();
    self_table.schema_name = "test".to_owned();
    self_table.table.name = "child".to_owned();
    let self_reference = ForeignKeyInfo {
        columns: vec!["id".to_owned()],
        referenced_table: "child".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        ..foreign_key
    };
    assert_eq!(
        check_foreign_key_definition(&self_table, &self_table, &self_reference),
        Err(ForeignKeyError::CannotAddForeignKey)
    );
}

/// 四种引用动作写入 catalog 后应保持一致，初始状态为 WriteOnly。
#[test]
fn referential_action_matrix_survives_catalog_insertion() {
    let mut catalog = catalog();
    let mut schema_version = 0;
    for (offset, action) in [
        ReferentialAction::Restrict,
        ReferentialAction::Cascade,
        ReferentialAction::SetNull,
        ReferentialAction::NoAction,
    ]
    .into_iter()
    .enumerate()
    {
        let mut foreign_key = foreign_key(&format!("fk_action_{offset}"));
        foreign_key.on_delete = action;
        foreign_key.on_update = action;
        advance_create_foreign_key(
            &mut catalog,
            "test",
            "child",
            &mut foreign_key,
            true,
            true,
            &mut schema_version,
            false,
        )
        .unwrap();
        assert_eq!(foreign_key.id, offset as i64 + 1);
        let stored = catalog
            .table("test", "child")
            .unwrap()
            .foreign_keys
            .iter()
            .find(|candidate| candidate.id == foreign_key.id)
            .unwrap();
        assert_eq!(stored.on_delete, action);
        assert_eq!(stored.on_update, action);
        assert_eq!(stored.state, SchemaState::WriteOnly);
    }
    assert_eq!(schema_version, 4);
    let child = catalog.table("test", "child").unwrap();
    assert_eq!(child.max_foreign_key_id, 4);
    assert_eq!(
        child
            .foreign_keys
            .iter()
            .map(|foreign_key| foreign_key.name.as_str())
            .collect::<Vec<_>>(),
        ["fk_action_0", "fk_action_1", "fk_action_2", "fk_action_3"]
    );
}

/// ADD 校验分支：子表缺索引、禁用外键检查、重名（大小写不敏感）、表不存在。
#[test]
fn add_foreign_key_validation_covers_disabled_duplicate_and_child_index_branches() {
    let mut catalog = catalog();
    let foreign_key = foreign_key("fk");
    catalog
        .table_mut("test", "child")
        .unwrap()
        .table
        .indices
        .clear();
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "child", &foreign_key, true),
        Err(ForeignKeyError::MissingIndex("fk".to_owned()))
    );

    // enabled=false 时跳过大部分校验。
    catalog.enabled = false;
    check_add_foreign_key_valid(&catalog, "test", "child", &foreign_key, true).unwrap();
    let mut existing = foreign_key.clone();
    existing.name = "Fk".to_owned();
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(existing);
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "TEST", "CHILD", &foreign_key, true),
        Err(ForeignKeyError::DuplicateName("fk".to_owned()))
    );
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "missing", &foreign_key, true),
        Err(ForeignKeyError::ColumnNotFound("missing".to_owned()))
    );
}

/// 前缀索引（IndexColumn.length 有值）不能作为父表引用索引。
#[test]
fn foreign_key_rejects_prefix_only_parent_index() {
    let mut catalog = catalog();
    catalog.table_mut("test", "parent").unwrap().table.indices[0].columns[0].length = Some(5);
    assert_eq!(
        check_add_foreign_key_valid(&catalog, "test", "child", &foreign_key("fk"), true,),
        Err(ForeignKeyError::MissingIndex("fk".to_owned()))
    );
}

/// SET NULL 动作要求子表外键列可空；NOT NULL 列应被拒绝。
#[test]
fn foreign_key_rejects_set_null_action_on_not_null_child_column() {
    let catalog = catalog();
    let parent = catalog.table("test", "parent").unwrap();
    let mut child = catalog.table("test", "child").unwrap().clone();
    child.table.columns[1].not_null = true;
    for (on_delete, on_update) in [
        (ReferentialAction::SetNull, ReferentialAction::Restrict),
        (ReferentialAction::Restrict, ReferentialAction::SetNull),
    ] {
        let mut foreign_key = foreign_key("fk");
        foreign_key.on_delete = on_delete;
        foreign_key.on_update = on_update;
        assert_eq!(
            check_foreign_key_definition(parent, &child, &foreign_key),
            Err(ForeignKeyError::CannotAddForeignKey),
            "ON DELETE and ON UPDATE SET NULL must both reject a NOT NULL child column"
        );
    }
}

/// 虚拟/生成列（generated）不可出现在外键任一侧。
#[test]
fn foreign_key_rejects_virtual_columns_on_either_side() {
    let catalog = catalog();
    let foreign_key = foreign_key("fk");

    let mut child = catalog.table("test", "child").unwrap().clone();
    child.table.columns[1].generated = true;
    assert_eq!(
        check_foreign_key_definition(
            catalog.table("test", "parent").unwrap(),
            &child,
            &foreign_key,
        ),
        Err(ForeignKeyError::CannotAddForeignKey),
        "a virtual child column must report the foreign-key definition error"
    );

    let mut parent = catalog.table("test", "parent").unwrap().clone();
    parent.table.columns[0].generated = true;
    assert_eq!(
        check_foreign_key_definition(
            &parent,
            catalog.table("test", "child").unwrap(),
            &foreign_key,
        ),
        Err(ForeignKeyError::CannotAddForeignKey),
        "a virtual parent column must report the foreign-key definition error"
    );
}

/// 类型兼容性：显示宽度、无符号位、字符集与排序规则均须匹配。
#[test]
fn foreign_key_type_compatibility_checks_width_sign_charset_and_collation() {
    let base = catalog();
    let foreign_key = foreign_key("fk");

    // Go 的 checkTableForeignKey 不比较 flen；整数显示宽度变化仍兼容。
    let mut wider_parent = base.table("test", "parent").unwrap().clone();
    wider_parent.table.columns[0].field_type.flen = 20;
    check_foreign_key_definition(
        &wider_parent,
        base.table("test", "child").unwrap(),
        &foreign_key,
    )
    .unwrap();

    let mut unsigned_parent = base.table("test", "parent").unwrap().clone();
    unsigned_parent.table.columns[0].field_type.unsigned = true;
    assert_eq!(
        check_foreign_key_definition(
            &unsigned_parent,
            base.table("test", "child").unwrap(),
            &foreign_key,
        ),
        Err(ForeignKeyError::IncompatibleColumns(
            "parent_id".to_owned(),
            "id".to_owned()
        ))
    );

    // charset/collation 任一不一致即拒绝。
    for (parent_charset, parent_collation) in
        [("utf8", "utf8_bin"), ("utf8mb4", "utf8mb4_general_ci")]
    {
        let mut parent = base.table("test", "parent").unwrap().clone();
        parent.table.columns[0] = column(1, "id", ColumnKind::Varchar);
        parent.table.columns[0].field_type.charset = parent_charset.to_owned();
        parent.table.columns[0].field_type.collation = parent_collation.to_owned();
        let mut child = base.table("test", "child").unwrap().clone();
        child.table.columns[1] = column(2, "parent_id", ColumnKind::Varchar);
        assert_eq!(
            check_foreign_key_definition(&parent, &child, &foreign_key),
            Err(ForeignKeyError::IncompatibleColumns(
                "parent_id".to_owned(),
                "id".to_owned()
            ))
        );
    }
}

/// Go 的列修改规则允许整数显示宽度变化，也允许字符串列加宽但不允许缩短。
#[test]
fn foreign_key_definition_and_modify_ignore_display_width_but_preserve_shrink_rules() {
    let base = catalog();
    let foreign_key = foreign_key("fk");

    let child = base.table("test", "child").unwrap();
    let parent = base.table("test", "parent").unwrap();
    let original = child.table.columns[1].clone();
    let mut wider_integer = original.clone();
    wider_integer.field_type.flen = original.field_type.flen + 10;

    // MySQL/TiDB integer display width is not part of FK type compatibility.
    check_modify_column_with_foreign_key(&base, "test", "child", &original, &wider_integer)
        .unwrap();

    let mut varchar_parent = parent.clone();
    varchar_parent.table.columns[0] = column(1, "id", ColumnKind::Varchar);
    varchar_parent.table.columns[0].field_type.flen = 10;
    let mut varchar_child = child.clone();
    varchar_child.table.columns[1] = column(2, "parent_id", ColumnKind::Varchar);
    varchar_child.table.columns[1].field_type.flen = 20;
    check_foreign_key_definition(&varchar_parent, &varchar_child, &foreign_key).unwrap();

    let mut catalog = base;
    catalog.table_mut("test", "parent").unwrap().table.columns[0] =
        varchar_parent.table.columns[0].clone();
    catalog.table_mut("test", "child").unwrap().table.columns[1] =
        varchar_child.table.columns[1].clone();
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key);

    let original_varchar = catalog.table("test", "child").unwrap().table.columns[1].clone();
    let mut wider_varchar = original_varchar.clone();
    wider_varchar.field_type.flen = 30;
    check_modify_column_with_foreign_key(
        &catalog,
        "test",
        "child",
        &original_varchar,
        &wider_varchar,
    )
    .unwrap();

    let mut shorter_varchar = original_varchar.clone();
    shorter_varchar.field_type.flen = 19;
    assert_eq!(
        check_modify_column_with_foreign_key(
            &catalog,
            "test",
            "child",
            &original_varchar,
            &shorter_varchar,
        ),
        Err(ForeignKeyError::IncompatibleColumns(
            "parent_id".to_owned(),
            "id".to_owned(),
        ))
    );
}

/// 自引用外键允许列集合不同的映射（如 (a,b) → (b,a)）。
#[test]
fn self_referencing_foreign_key_allows_distinct_column_mapping() {
    let mut self_table = table(
        1,
        "test",
        "self_table",
        vec![
            column(1, "a", ColumnKind::Integer),
            column(2, "b", ColumnKind::Integer),
        ],
        vec![index(1, "a_b", &["a", "b"]), index(2, "b_a", &["b", "a"])],
    );
    self_table.table.name = "self_table".to_owned();
    let foreign_key = ForeignKeyInfo {
        columns: vec!["a".to_owned(), "b".to_owned()],
        referenced_table: "self_table".to_owned(),
        referenced_columns: vec!["b".to_owned(), "a".to_owned()],
        ..foreign_key("fk")
    };
    check_foreign_key_definition(&self_table, &self_table, &foreign_key).unwrap();
}

/// `referred_foreign_keys` 与引用检查对库/表名大小写不敏感。
#[test]
fn foreign_key_catalog_lookup_and_referred_metadata_are_case_insensitive() {
    let mut catalog = catalog();
    let mut first = foreign_key("fk_first");
    first.state = SchemaState::Public;
    catalog
        .table_mut("TEST", "CHILD")
        .unwrap()
        .foreign_keys
        .push(first);
    catalog.add_table(table(
        3,
        "Other",
        "Second_Child",
        vec![column(1, "parent_id", ColumnKind::Integer)],
        vec![index(3, "child_parent_id", &["parent_id"])],
    ));
    let mut second = foreign_key("fk_second");
    second.referenced_schema = "TEST".to_owned();
    second.referenced_table = "PARENT".to_owned();
    second.state = SchemaState::Public;
    catalog
        .table_mut("other", "second_child")
        .unwrap()
        .foreign_keys
        .push(second);

    // 收集所有引用 parent 的子表外键，排序后断言完整集合。
    let mut referred = catalog
        .referred_foreign_keys("tEsT", "pArEnT")
        .into_iter()
        .map(|(child, foreign_key)| {
            (
                child.schema_name.clone(),
                child.table.name.clone(),
                foreign_key.name.clone(),
            )
        })
        .collect::<Vec<_>>();
    referred.sort();
    assert_eq!(
        referred,
        vec![
            (
                "Other".to_owned(),
                "Second_Child".to_owned(),
                "fk_second".to_owned()
            ),
            ("test".to_owned(), "child".to_owned(), "fk_first".to_owned()),
        ]
    );
    // exclude 仅放行部分子表时，仍被另一子表引用则失败。
    assert_eq!(
        check_table_has_foreign_key_referred(
            &catalog,
            "test",
            "parent",
            &[("test".to_owned(), "child".to_owned())],
            true,
        ),
        Err(ForeignKeyError::ParentIsReferenced {
            table: "parent".to_owned(),
            foreign_key: "fk_second".to_owned(),
            child_table: "Second_Child".to_owned(),
        })
    );
    check_table_has_foreign_key_referred(
        &catalog,
        "test",
        "parent",
        &[
            ("test".to_owned(), "child".to_owned()),
            ("other".to_owned(), "second_child".to_owned()),
        ],
        true,
    )
    .unwrap();
}

/// 索引/列保护覆盖子表、父表、冗余无关索引与无关列路径。
#[test]
fn index_and_column_guards_cover_child_parent_redundant_and_unrelated_paths() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk");
    foreign_key.state = SchemaState::Public;
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key);

    assert_eq!(
        check_index_needed_in_foreign_key(&catalog, "test", "child", 2),
        Err(ForeignKeyError::IndexNeeded("child_parent_id".to_owned()))
    );
    assert_eq!(
        check_index_needed_in_foreign_key(&catalog, "test", "child", 99),
        Err(ForeignKeyError::IndexNeeded("99".to_owned()))
    );
    // 与外键无关的索引可以删除。
    catalog
        .table_mut("test", "child")
        .unwrap()
        .table
        .indices
        .push(index(4, "child_id", &["id"]));
    check_index_needed_in_foreign_key(&catalog, "test", "child", 4).unwrap();

    check_drop_column_with_foreign_key(&catalog, "test", "child", "id").unwrap();
    assert_eq!(
        check_drop_column_with_foreign_key(&catalog, "test", "child", "parent_id"),
        Err(ForeignKeyError::ColumnNeeded(
            "parent_id".to_owned(),
            "fk".to_owned()
        ))
    );
    assert_eq!(
        check_drop_column_with_foreign_key(&catalog, "test", "parent", "id"),
        Err(ForeignKeyError::ColumnNeeded(
            "id".to_owned(),
            "fk".to_owned()
        ))
    );
}

/// MODIFY COLUMN 校验：相同类型可过；类型变更、禁用检查、缺失父表/表路径。
#[test]
fn column_change_validation_covers_child_parent_disabled_and_missing_metadata() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk");
    foreign_key.state = SchemaState::Public;
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key);
    let child_original = catalog.table("test", "child").unwrap().table.columns[1].clone();
    let parent_original = catalog.table("test", "parent").unwrap().table.columns[0].clone();

    check_modify_column_with_foreign_key(
        &catalog,
        "test",
        "child",
        &child_original,
        &child_original,
    )
    .unwrap();
    check_modify_column_with_foreign_key(
        &catalog,
        "test",
        "parent",
        &parent_original,
        &parent_original,
    )
    .unwrap();

    // INT → VARCHAR 破坏外键类型兼容性。
    let child_varchar = column(2, "parent_id", ColumnKind::Varchar);
    assert_eq!(
        check_modify_column_with_foreign_key(
            &catalog,
            "test",
            "child",
            &child_original,
            &child_varchar,
        ),
        Err(ForeignKeyError::IncompatibleColumns(
            "parent_id".to_owned(),
            "id".to_owned()
        ))
    );
    let parent_varchar = column(1, "id", ColumnKind::Varchar);
    assert_eq!(
        check_modify_column_with_foreign_key(
            &catalog,
            "test",
            "parent",
            &parent_original,
            &parent_varchar,
        ),
        Err(ForeignKeyError::IncompatibleColumns(
            "parent_id".to_owned(),
            "id".to_owned()
        ))
    );

    // 关闭外键检查后允许不兼容变更。
    catalog.enabled = false;
    check_modify_column_with_foreign_key(
        &catalog,
        "test",
        "child",
        &child_original,
        &child_varchar,
    )
    .unwrap();
    catalog.enabled = true;
    catalog
        .tables
        .remove(&("test".to_owned(), "parent".to_owned()));
    assert_eq!(
        check_modify_column_with_foreign_key(
            &catalog,
            "test",
            "child",
            &child_original,
            &child_original,
        ),
        Err(ForeignKeyError::CannotOpenParent("parent".to_owned()))
    );
    assert_eq!(
        check_modify_column_with_foreign_key(
            &catalog,
            "test",
            "missing",
            &child_original,
            &child_original,
        ),
        Err(ForeignKeyError::ColumnNotFound("missing".to_owned()))
    );
}

/// 可接受的列变更：不可缩短到小于原长或关联列长度；加宽或改类型规则另判。
#[test]
fn acceptable_column_change_matrix_preserves_length_and_type_rules() {
    let mut original = column(1, "c", ColumnKind::Varchar);
    original.field_type.flen = 32;
    let mut related = column(2, "r", ColumnKind::Varchar);
    related.field_type.flen = 24;

    let mut shorter_than_original = original.clone();
    shorter_than_original.field_type.flen = 31;
    assert!(!is_acceptable_foreign_key_column_change(
        &shorter_than_original,
        &original,
        &related,
    ));
    let mut shorter_than_related = original.clone();
    shorter_than_related.field_type.flen = 23;
    assert!(!is_acceptable_foreign_key_column_change(
        &shorter_than_related,
        &original,
        &related,
    ));
    let mut wider = original.clone();
    wider.field_type.flen = 64;
    assert!(is_acceptable_foreign_key_column_change(
        &wider, &original, &related,
    ));

    let integer = column(3, "i", ColumnKind::Integer);
    assert!(is_acceptable_foreign_key_column_change(
        &integer, &original, &related,
    ));
}

/// 复合外键检查 SQL：过滤 NULL，并按列序做 NOT IN 反连接探测。
#[test]
fn foreign_key_check_sql_preserves_composite_column_order_and_null_filtering() {
    let foreign_key = ForeignKeyInfo {
        columns: vec!["tenant".to_owned(), "parent_id".to_owned()],
        referenced_schema: "parent_schema".to_owned(),
        referenced_table: "parent_table".to_owned(),
        referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
        ..foreign_key("fk")
    };
    assert_eq!(
        build_foreign_key_check_sql("child_schema", "child_table", &foreign_key),
        "SELECT 1 FROM `child_schema`.`child_table` WHERE `tenant` IS NOT NULL AND `parent_id` IS NOT NULL AND (`tenant`,`parent_id`) NOT IN (SELECT `tenant`,`id` FROM `parent_schema`.`parent_table`) LIMIT 1"
    );
}

/// 状态机异常：catalog 中找不到对应 FK，或已处于 Public 时不可再 advance。
#[test]
fn create_foreign_key_state_machine_reports_missing_and_invalid_states() {
    let mut catalog = catalog();
    let mut foreign_key = foreign_key("fk");
    foreign_key.id = 99;
    foreign_key.state = SchemaState::WriteOnly;
    let mut schema_version = 0;
    assert_eq!(
        advance_create_foreign_key(
            &mut catalog,
            "test",
            "child",
            &mut foreign_key,
            true,
            true,
            &mut schema_version,
            false,
        ),
        Err(ForeignKeyError::ForeignKeyNotFound("fk".to_owned()))
    );

    foreign_key.id = 1;
    foreign_key.state = SchemaState::Public;
    catalog
        .table_mut("test", "child")
        .unwrap()
        .foreign_keys
        .push(foreign_key.clone());
    assert_eq!(
        advance_create_foreign_key(
            &mut catalog,
            "test",
            "child",
            &mut foreign_key,
            true,
            true,
            &mut schema_version,
            false,
        ),
        Err(ForeignKeyError::InvalidState(SchemaState::Public))
    );
    assert_eq!(schema_version, 0);
}
