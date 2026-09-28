// Copyright 2026 AsterSQL.

// InfoSchema 接口与请求上下文的轻量单元测试。
//
// 覆盖：`TableItem` 克隆保留库表标识；`InfoSchema` 作为 object-safe trait 可用；
// `Background` / `TODO` 根上下文永不到期、不取消、无键值。

use super::*;

/// 验证 TableItem 克隆后仍保留库名与表名（大小写不敏感小写形式）。
#[test]
fn table_item_preserves_database_and_table_identity() {
    let item = TableItem {
        DBName: CiString::new("analytics"),
        TableName: CiString::new("events"),
    };
    let copied = item.clone();

    assert_eq!(copied.DBName.lower, "analytics");
    assert_eq!(copied.TableName.lower, "events");
}

/// 编译期断言 InfoSchema 可作为 `dyn Trait`（object-safe）传递。
#[test]
fn infoschema_remains_an_object_safe_interface() {
    fn accept(_: &dyn InfoSchema) {}
    let _ = accept;
}

/// 验证 Background / TODO 根上下文：无 deadline、未完成、无错误、无键值。
#[test]
fn root_contexts_never_finish_or_carry_values() {
    let key = String::from("request-key");
    for context in [Background(), TODO()] {
        assert_eq!(context.deadline(), None);
        assert!(!context.is_done());
        assert_eq!(context.error(), None);
        assert!(context.value(&key).is_none());
    }
}
