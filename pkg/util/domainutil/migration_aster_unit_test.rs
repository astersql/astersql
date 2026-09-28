// Copyright 2026 AsterSQL.
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

// `domainutil` 迁移期单元测试：对齐 Go 修复模式状态机行为。
//
// 覆盖：repair mode 开关与表名小写规范化、按库筛选必须加载的表 ID、
// 修复缓存的写入/查找语义，以及移除表后关闭空修复模式；并校验
// `repairKeyType` 的字符串键与 Go 常量一致。

use std::collections::HashMap;
use std::sync::Arc;

use super::{init, model, repairKeyType};

/// 构造带原始名/小写名的测试用 `DBInfo`。
fn db_info(id: i64, original_name: &str, lower_name: &str) -> model::DBInfo {
    let mut db = model::DBInfo::default();
    db.ID = id;
    db.Name.O = original_name.to_owned();
    db.Name.L = lower_name.to_owned();
    db
}

/// 构造带原始名/小写名的测试用 `TableInfo`（`Arc` 包装便于缓存指针比较）。
fn table_info(id: i64, original_name: &str, lower_name: &str) -> Arc<model::TableInfo> {
    let mut table = model::TableInfo::default();
    table.ID = id;
    table.Name.O = original_name.to_owned();
    table.Name.L = lower_name.to_owned();
    Arc::new(table)
}

/// 验证 SetRepairMode / SetRepairTableList 的状态转换与表名小写化。
#[test]
fn repair_mode_and_table_list_follow_go_state_transitions() {
    let mut info = init();
    assert!(!info.InRepairMode());
    assert!(info.GetRepairTableList().is_empty());

    info.SetRepairMode(true);
    info.SetRepairTableList(vec!["TeSt.Orders".to_owned(), "test.CUSTOMERS".to_owned()]);

    assert!(info.InRepairMode());
    assert_eq!(
        info.GetRepairTableList(),
        &["test.orders".to_owned(), "test.customers".to_owned()]
    );
}

/// 验证按库名前缀筛选待修复表，并以大小写不敏感方式反查表 ID。
#[test]
fn must_load_ids_match_names_case_insensitively_within_the_database() {
    let mut info = init();
    info.SetRepairTableList(vec![
        "test.orders".to_owned(),
        "TEST.Customers".to_owned(),
        "other.orders".to_owned(),
    ]);

    let table_name_to_id = HashMap::from([
        ("Orders".to_owned(), 11),
        ("CUSTOMERS".to_owned(), 12),
        ("Unlisted".to_owned(), 13),
    ]);
    let mut ids = info.GetMustLoadRepairTableListByDB("test", &table_name_to_id);
    ids.sort_unstable();

    assert_eq!(ids, vec![11, 12]);
}

/// 验证 CheckAndFetchRepairedTable 缓存与按名查找（含仅命中库）行为。
#[test]
fn fetching_and_lookup_preserve_the_go_repair_cache_behavior() {
    let mut info = init();
    let stale = table_info(1, "stale", "stale");
    let orders = table_info(2, "Orders", "orders");
    let customers = table_info(3, "Customers", "customers");
    let mut db = db_info(10, "TeSt", "test");
    db.Deprecated.Tables.push(stale);

    info.SetRepairTableList(vec!["TEST.Orders".to_owned(), "test.customers".to_owned()]);
    assert!(!info.CheckAndFetchRepairedTable(&db, Arc::clone(&orders)));

    info.SetRepairMode(true);
    assert!(!info.CheckAndFetchRepairedTable(&db, table_info(4, "Ignored", "ignored")));
    assert!(info.CheckAndFetchRepairedTable(&db, Arc::clone(&orders)));
    assert!(info.CheckAndFetchRepairedTable(&db, Arc::clone(&customers)));

    let (found_table, found_db) = info.GetRepairedTableInfoByTableName("test", "orders");
    assert!(Arc::ptr_eq(
        found_table.expect("orders must be cached"),
        &orders
    ));
    let found_db = found_db.expect("database must be cached");
    assert_eq!(found_db.ID, 10);
    assert_eq!(found_db.Deprecated.Tables.len(), 2);

    let (missing_table, same_db) = info.GetRepairedTableInfoByTableName("test", "missing");
    assert!(missing_table.is_none());
    assert_eq!(same_db.expect("database remains visible").ID, 10);

    let (missing_table, missing_db) = info.GetRepairedTableInfoByTableName("missing", "orders");
    assert!(missing_table.is_none());
    assert!(missing_db.is_none());
}

/// 验证 RemoveFromRepairInfo 同步更新列表与 map，并在清空后关闭 repairMode。
#[test]
fn removing_tables_updates_both_collections_and_disables_empty_repair_mode() {
    let mut info = init();
    let sales = db_info(10, "Sales", "sales");
    let archive = db_info(20, "Archive", "archive");
    info.SetRepairMode(true);
    info.SetRepairTableList(vec![
        "Sales.Orders".to_owned(),
        "sales.Customers".to_owned(),
        "Archive.Events".to_owned(),
    ]);
    assert!(info.CheckAndFetchRepairedTable(&sales, table_info(11, "Orders", "orders")));
    assert!(info.CheckAndFetchRepairedTable(&sales, table_info(12, "Customers", "customers")));
    assert!(info.CheckAndFetchRepairedTable(&archive, table_info(21, "Events", "events")));

    info.RemoveFromRepairInfo("sales", "orders");
    assert_eq!(
        info.GetRepairTableList(),
        &["sales.customers".to_owned(), "archive.events".to_owned()]
    );
    let (orders, sales_db) = info.GetRepairedTableInfoByTableName("sales", "orders");
    assert!(orders.is_none());
    assert!(sales_db.is_some());
    assert!(info.InRepairMode());

    info.RemoveFromRepairInfo("sales", "customers");
    assert!(
        info.GetRepairedTableInfoByTableName("sales", "customers")
            .1
            .is_none()
    );
    assert!(info.InRepairMode());

    info.RemoveFromRepairInfo("archive", "events");
    assert!(info.GetRepairTableList().is_empty());
    assert!(!info.InRepairMode());
}

/// 验证 sessionCtx 缓存键字符串与 Go 常量一致。
#[test]
fn repair_key_strings_match_go_constants() {
    assert_eq!(repairKeyType::RepairedTable.String(), "RepairedTable");
    assert_eq!(repairKeyType::RepairedDatabase.String(), "RepairedDatabase");
}
