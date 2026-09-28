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

// `DBInfo` / `TableInfo` 迁移对齐单元测试。
//
// 校验默认值、按表名索引、以及 Go map/指针复制后的共享语义。

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, RwLock};

use crate::{DBInfo, TableInfo, TableInfoMapRef, model};

/// 零值应与 Go 结构体零值语义一致：ID 为 0，字符串与集合为空，指针为 None。
#[test]
fn zero_values_match_go_struct_behavior() {
    let mut db = DBInfo::default();
    assert_eq!(db.ID, 0);
    assert!(db.Name.is_empty());
    assert!(db.Tables.is_none());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            db.Tables
                .as_mut()
                .expect("writing a nil Go map")
                .write()
                .expect("table map lock")
                .insert(
                    "orders".to_owned(),
                    Arc::new(RwLock::new(TableInfo::default())),
                );
        }))
        .is_err(),
        "writing a Go nil map must fail until it is explicitly initialized"
    );

    let table = TableInfo::default();
    assert_eq!(table.ID, 0);
    assert!(table.DB.is_empty());
    assert!(table.Name.is_empty());
    assert!(table.Core.is_none());
    assert!(table.Desired.is_none());
}

/// 数据库内的表应按名称可检索，缺失名返回 None。
#[test]
fn database_keeps_tables_addressable_by_name() {
    let table = TableInfo {
        ID: 17,
        DB: "sales".to_owned(),
        Name: "orders".to_owned(),
        ..Default::default()
    };
    let mut tables = HashMap::new();
    tables.insert("orders".to_owned(), Arc::new(RwLock::new(table)));
    let tables: TableInfoMapRef = Arc::new(RwLock::new(tables));

    let db = DBInfo {
        ID: 9,
        Name: "sales".to_owned(),
        Tables: Some(Arc::clone(&tables)),
    };

    let table_map = db
        .Tables
        .as_ref()
        .expect("initialized table map")
        .read()
        .expect("table map lock");
    let orders = table_map
        .get("orders")
        .expect("orders table")
        .read()
        .expect("orders lock");
    assert_eq!(orders.ID, 17);
    assert_eq!(orders.DB, "sales");
    assert_eq!(orders.Name, "orders");
    assert!(table_map.get("missing").is_none());
    drop(orders);
    drop(table_map);

    let cloned = db.clone();
    cloned
        .Tables
        .as_ref()
        .expect("cloned table map")
        .write()
        .expect("cloned table map lock")
        .insert(
            "customers".to_owned(),
            Arc::new(RwLock::new(TableInfo::default())),
        );
    assert!(
        tables
            .read()
            .expect("original table map lock")
            .contains_key("customers"),
        "copying a Go map preserves its shared backing storage"
    );
}

/// Go 指针在结构复制后仍指向同一对象，Core/Desired 也允许显式别名。
#[test]
fn copied_and_dual_metadata_pointers_preserve_aliases() {
    let mut core = model::TableInfo::default();
    core.ID = 23;
    core.Columns.push(Default::default());
    core.Indices.push(Default::default());
    core.Constraints.push(Default::default());
    core.Partition = Some(Default::default());
    let shared = Arc::new(RwLock::new(core));

    let table = TableInfo {
        Core: Some(Arc::clone(&shared)),
        Desired: Some(Arc::clone(&shared)),
        ..Default::default()
    };
    table
        .Desired
        .as_ref()
        .expect("desired")
        .write()
        .expect("desired lock")
        .ID = 29;
    assert!(Arc::ptr_eq(
        table.Core.as_ref().expect("core"),
        table.Desired.as_ref().expect("desired")
    ));
    assert_eq!(
        table
            .Core
            .as_ref()
            .expect("core")
            .read()
            .expect("core lock")
            .ID,
        29,
        "Core and Desired may be the same Go pointer"
    );
    let complete_model = table
        .Core
        .as_ref()
        .expect("core")
        .read()
        .expect("core lock");
    assert_eq!(complete_model.Columns.len(), 1);
    assert_eq!(complete_model.Indices.len(), 1);
    assert_eq!(complete_model.Constraints.len(), 1);
    assert!(complete_model.Partition.is_some());
    drop(complete_model);

    let cloned = table.clone();
    cloned
        .Core
        .as_ref()
        .expect("cloned core")
        .write()
        .expect("cloned core lock")
        .ID = 31;
    assert!(Arc::ptr_eq(
        table.Core.as_ref().expect("original core"),
        cloned.Core.as_ref().expect("cloned core")
    ));
    assert_eq!(
        table
            .Core
            .as_ref()
            .expect("original core")
            .read()
            .expect("original core lock")
            .ID,
        31,
        "copying the containing Go struct preserves pointer identity"
    );
}
