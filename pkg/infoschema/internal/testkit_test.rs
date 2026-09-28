// Copyright 2026 AsterSQL.

// 验证 infoschema 内部测试工具与 Go `meta.Mutator` 的元数据边界行为一致。
//
// 重点覆盖缺失对象删除、默认资源组更新和非法零 ID，防止内存模拟存储偏离
// 生产元数据层的存在性规则。

use super::{
    AddDB, AddResourceGroup, AddTable, CreatePolicy, DbInfo, DropDB, DropPolicy, DropResourceGroup,
    PolicyInfo, ResourceGroupInfo, TableInfo, TestStore, UpdateResourceGroup, UpdateTable,
};

#[test]
fn metadata_helpers_match_meta_mutator_existence_rules() {
    let store = TestStore::new(Vec::new()).expect("create mock store");

    // Go 元数据层的删除操作在 KV 层具有幂等性：删除不存在的哈希项不会报错。
    let database = DbInfo {
        id: 101,
        ..DbInfo::default()
    };
    assert!(DropDB(&store, &database).is_ok());

    let missing_group = ResourceGroupInfo {
        id: 200,
        ..ResourceGroupInfo::default()
    };
    assert!(DropResourceGroup(&store, &missing_group).is_ok());

    let missing_policy = PolicyInfo {
        id: 300,
        ..PolicyInfo::default()
    };
    assert!(DropPolicy(&store, &missing_policy).is_ok());

    // 默认资源组由系统合成，可能尚未持久化，因此无需先 Add 就可以直接更新。
    let default_group = ResourceGroupInfo {
        id: 1,
        ..ResourceGroupInfo::default()
    };
    assert!(UpdateResourceGroup(&store, &default_group).is_ok());

    // Go Mutator 在访问元数据之前会拒绝零 ID，模拟实现也必须保持这一约束。
    let zero_group = ResourceGroupInfo::default();
    assert!(AddResourceGroup(&store, &zero_group).is_err());
    let zero_policy = PolicyInfo::default();
    assert!(CreatePolicy(&store, &zero_policy).is_err());
}

#[test]
fn update_table_advances_revision_only_after_existence_checks() {
    let store = TestStore::new(Vec::new()).expect("create mock store");
    let database = DbInfo {
        id: 101,
        ..DbInfo::default()
    };
    AddDB(&store, &database).expect("add database");

    let mut table = TableInfo {
        id: 102,
        revision: 7,
        ..TableInfo::default()
    };
    AddTable(&store, database.id, &table).expect("add table");
    UpdateTable(&store, &database, &mut table).expect("update table");
    assert_eq!(table.revision, 8);

    let mut missing = TableInfo {
        id: 103,
        revision: 9,
        ..TableInfo::default()
    };
    assert!(UpdateTable(&store, &database, &mut missing).is_err());
    assert_eq!(missing.revision, 9);
}
